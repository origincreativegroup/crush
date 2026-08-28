//! Resumable, verified model downloads.

use anyhow::{bail, ensure, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/origincreativegroup/crush/releases/download/models-v1/manifest.json";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Manifest {
    pub dim: usize,
    pub embedding_sha256: String,
    pub files: BTreeMap<String, ModelFile>,
    pub model_name: String,
    pub preprocess_version: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ModelFile {
    pub bytes: u64,
    pub sha256: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub name: String,
    pub downloaded: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelStatus {
    Present,
    Missing,
    ShaMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCheck {
    pub name: String,
    pub status: ModelStatus,
}

pub fn bundled_manifest() -> anyhow::Result<Manifest> {
    serde_json::from_str(include_str!("../model-manifest-v1.json"))
        .context("bundled model manifest is invalid")
}

pub fn ensure(
    models_dir: &Path,
    manifest_url: &str,
    progress: impl Fn(Progress),
) -> anyhow::Result<Manifest> {
    fs::create_dir_all(models_dir)?;
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build()
        .new_agent();
    let manifest = fetch_manifest(&agent, manifest_url)?;
    validate_manifest(&manifest)?;
    let base_url = manifest_url
        .rsplit_once('/')
        .map(|(base, _)| base)
        .context("manifest URL must contain a path separator")?;

    for (name, expected) in &manifest.files {
        let destination = models_dir.join(name);
        if verify_file(&destination, expected)? {
            progress(Progress {
                name: name.clone(),
                downloaded: expected.bytes,
                total: expected.bytes,
            });
            continue;
        }
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        let url = expected
            .url
            .clone()
            .unwrap_or_else(|| format!("{base_url}/{name}"));
        download_with_retries(&agent, &url, name, expected, &destination, &progress)?;
    }
    Ok(manifest)
}

pub fn inspect(models_dir: &Path, manifest: &Manifest) -> anyhow::Result<Vec<ModelCheck>> {
    manifest
        .files
        .iter()
        .map(|(name, expected)| {
            let path = models_dir.join(name);
            let status = if !path.is_file() {
                ModelStatus::Missing
            } else if verify_file(&path, expected)? {
                ModelStatus::Present
            } else {
                ModelStatus::ShaMismatch
            };
            Ok(ModelCheck {
                name: name.clone(),
                status,
            })
        })
        .collect()
}

fn fetch_manifest(agent: &ureq::Agent, url: &str) -> anyhow::Result<Manifest> {
    let mut last_error = None;
    for attempt in 0..3 {
        let result = (|| -> anyhow::Result<Manifest> {
            let mut response = agent.get(url).call()?;
            let json = response.body_mut().read_to_string()?;
            Ok(serde_json::from_str(&json)?)
        })();
        match result {
            Ok(manifest) => return Ok(manifest),
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(100 * (1 << attempt)));
    }
    Err(last_error.expect("three attempts always record an error"))
        .context("manifest download failed after three attempts")
}

fn validate_manifest(manifest: &Manifest) -> anyhow::Result<()> {
    ensure!(
        !manifest.files.is_empty(),
        "model manifest contains no files"
    );
    ensure!(
        manifest.embedding_sha256.len() == 64
            && manifest
                .embedding_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "model manifest has an invalid embedding sha256"
    );
    for (name, file) in &manifest.files {
        ensure!(
            Path::new(name).components().count() == 1 && name != "." && name != "..",
            "unsafe model filename in manifest: {name}"
        );
        ensure!(file.bytes > 0, "model {name} has an empty byte count");
        ensure!(
            file.sha256.len() == 64 && file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "model {name} has an invalid sha256"
        );
    }
    Ok(())
}

fn download_with_retries(
    agent: &ureq::Agent,
    url: &str,
    name: &str,
    expected: &ModelFile,
    destination: &Path,
    progress: &impl Fn(Progress),
) -> anyhow::Result<()> {
    let part = part_path(destination);
    let mut last_error = None;
    for attempt in 0..3 {
        match download_once(agent, url, name, expected, &part, progress).and_then(|()| {
            ensure!(verify_file(&part, expected)?, "sha256 mismatch for {name}");
            fs::rename(&part, destination)?;
            Ok(())
        }) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if part
                    .metadata()
                    .is_ok_and(|metadata| metadata.len() >= expected.bytes)
                {
                    fs::remove_file(&part).ok();
                }
            }
        }
        thread::sleep(Duration::from_millis(100 * (1 << attempt)));
    }
    Err(last_error.context(format!("model {name} failed after three attempts"))?)
}

fn download_once(
    agent: &ureq::Agent,
    url: &str,
    name: &str,
    expected: &ModelFile,
    part: &Path,
    progress: &impl Fn(Progress),
) -> anyhow::Result<()> {
    let offset = part.metadata().map_or(0, |metadata| metadata.len());
    if offset > expected.bytes {
        fs::remove_file(part)?;
        bail!("partial model {name} exceeded its expected size");
    }
    let mut request = agent.get(url);
    if offset > 0 {
        request = request.header("Range", &format!("bytes={offset}-"));
    }
    let mut response = request.call()?;
    let resumed = offset > 0 && response.status() == 206;
    let mut file = if resumed {
        OpenOptions::new().append(true).open(part)?
    } else {
        File::create(part)?
    };
    let mut downloaded = if resumed { offset } else { 0 };
    let mut last_reported = downloaded;
    progress(Progress {
        name: name.to_owned(),
        downloaded,
        total: expected.bytes,
    });
    let mut reader = response.body_mut().as_reader();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count])?;
        downloaded += count as u64;
        ensure!(
            downloaded <= expected.bytes,
            "model {name} exceeded expected size"
        );
        if downloaded == expected.bytes
            || downloaded.saturating_sub(last_reported) >= 8 * 1024 * 1024
        {
            progress(Progress {
                name: name.to_owned(),
                downloaded,
                total: expected.bytes,
            });
            last_reported = downloaded;
        }
    }
    file.sync_all()?;
    ensure!(
        downloaded == expected.bytes,
        "model {name} download was truncated"
    );
    Ok(())
}

fn verify_file(path: &Path, expected: &ModelFile) -> anyhow::Result<bool> {
    if !path.is_file() || path.metadata()?.len() != expected.bytes {
        return Ok(false);
    }
    Ok(sha256(path)? == expected.sha256)
}

fn sha256(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let digest = digest.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

fn part_path(destination: &Path) -> PathBuf {
    let mut value = destination.as_os_str().to_owned();
    value.push(".part");
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
    };

    struct Server {
        url: String,
        ranges: Arc<Mutex<Vec<Option<u64>>>>,
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    fn server(content: Vec<u8>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("http://{address}");
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let server_ranges = Arc::clone(&ranges);
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let digest = {
            let mut digest = Sha256::new();
            digest.update(&content);
            let digest = digest.finalize();
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let manifest = serde_json::json!({
            "dim": 512,
            "embedding_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "files": {"model.bin": {"bytes": content.len(), "sha256": digest}},
            "model_name": "test-model",
            "preprocess_version": 1
        })
        .to_string()
        .into_bytes();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if server_stop.load(Ordering::Relaxed) {
                    break;
                }
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut range = None;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                    if header.to_ascii_lowercase().starts_with("range: bytes=") {
                        range = header
                            .split_once('=')
                            .and_then(|(_, value)| value.trim().trim_end_matches('-').parse().ok());
                    }
                }
                if request.contains("/manifest.json") {
                    respond(&mut stream, 200, &manifest);
                } else if request.contains("/model.bin") {
                    server_ranges.lock().unwrap().push(range);
                    let offset = range.unwrap_or(0) as usize;
                    respond(
                        &mut stream,
                        if range.is_some() { 206 } else { 200 },
                        &content[offset..],
                    );
                } else {
                    respond(&mut stream, 404, b"missing");
                }
            }
        });
        Server {
            url,
            ranges,
            stop,
            thread: Some(thread),
        }
    }

    fn respond(stream: &mut TcpStream, status: u16, body: &[u8]) {
        let reason = if status == 206 {
            "Partial Content"
        } else if status == 200 {
            "OK"
        } else {
            "Not Found"
        };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    }

    #[test]
    fn fresh_resume_and_corrupt_downloads_finish_verified() {
        let content = b"verified model payload".to_vec();
        let server = server(content.clone());
        let temporary = tempfile::tempdir().unwrap();
        let manifest_url = format!("{}/manifest.json", server.url);
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = Arc::clone(&events);
        let manifest = ensure(temporary.path(), &manifest_url, move |event| {
            callback_events.lock().unwrap().push(event)
        })
        .unwrap();
        assert_eq!(
            fs::read(temporary.path().join("model.bin")).unwrap(),
            content
        );
        assert!(!events.lock().unwrap().is_empty());
        assert_eq!(
            inspect(temporary.path(), &manifest).unwrap()[0].status,
            ModelStatus::Present
        );

        fs::remove_file(temporary.path().join("model.bin")).unwrap();
        fs::write(temporary.path().join("model.bin.part"), &content[..5]).unwrap();
        ensure(temporary.path(), &manifest_url, |_| {}).unwrap();
        assert_eq!(server.ranges.lock().unwrap().last(), Some(&Some(5)));

        fs::write(
            temporary.path().join("model.bin"),
            vec![b'x'; content.len()],
        )
        .unwrap();
        ensure(temporary.path(), &manifest_url, |_| {}).unwrap();
        assert_eq!(
            fs::read(temporary.path().join("model.bin")).unwrap(),
            content
        );
        assert_eq!(server.ranges.lock().unwrap().last(), Some(&None));
    }

    #[test]
    fn bundled_manifest_has_the_complete_models_v1_release() {
        let manifest = bundled_manifest().unwrap();
        validate_manifest(&manifest).unwrap();
        assert_eq!(manifest.dim, 512);
        assert_eq!(manifest.files.len(), 5);
        for name in [
            "clip-image.onnx",
            "clip-text.onnx",
            "bpe_simple_vocab_16e6.txt.gz",
            "ggml-base.bin",
            "ggml-small.bin",
        ] {
            assert!(manifest.files.contains_key(name), "missing {name}");
        }
    }
}
