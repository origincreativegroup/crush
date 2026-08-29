//! Capability-aware, non-destructive still-image decoding and derivative generation.

use std::collections::BTreeSet;
use std::fs::File;
#[cfg(target_os = "macos")]
use std::io::BufReader;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::sync::OnceLock;

use anyhow::{bail, ensure, Context};
use chrono::{DateTime, NaiveDateTime, Utc};
use crush_core::cancellation::CancellationToken;
use crush_store::PhotoProxyProvenance;
use exif::{In, Reader as ExifReader, Tag, Value};
use image::codecs::jpeg::JpegEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader};
use serde_json::{json, Map, Value as JsonValue};
use sha2::{Digest, Sha256};

#[cfg(target_os = "macos")]
use std::io::Read;
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;
#[cfg(target_os = "macos")]
use std::process::Stdio;
#[cfg(target_os = "macos")]
use std::thread;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

/// Hard per-invocation ceiling for every sips (macOS ImageIO) subprocess.
#[cfg(target_os = "macos")]
const SIPS_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(target_os = "macos")]
const SIPS_POLL_INTERVAL: Duration = Duration::from_millis(20);

pub const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "tif", "tiff", "heic", "heif", "dng", "cr2", "cr3", "nef", "arw", "orf",
    "raf", "rw2",
];

const IMAGE_RS_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff"];
const IMAGE_IO_EXTENSIONS: &[&str] = &[
    "heic", "heif", "dng", "cr2", "cr3", "nef", "arw", "orf", "raf", "rw2",
];
#[cfg(target_os = "macos")]
static MACOS_IMAGEIO_FORMATS: OnceLock<BTreeSet<String>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoCapability {
    pub extension: &'static str,
    pub decoder: &'static str,
    pub available: bool,
    pub reason: String,
}

#[derive(Debug)]
pub struct DecodedPhoto {
    pub image: DynamicImage,
    pub source_format: String,
    pub decoder: String,
    pub proxy_provenance: PhotoProxyProvenance,
    pub orientation: Option<i64>,
    pub orientation_applied: bool,
    pub captured_at: Option<DateTime<Utc>>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub bit_depth: Option<i64>,
    pub color_space: Option<String>,
    pub icc_profile_name: Option<String>,
    pub icc_profile_sha256: Option<String>,
    /// Profile attached to color-managed derivatives; profile bytes stay out of SQLite.
    pub icc_profile: Option<Vec<u8>>,
    pub exposure_json: String,
    pub gps_present: bool,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivative {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
}

#[derive(Default)]
struct ExtractedExif {
    orientation: Option<u8>,
    captured_at: Option<DateTime<Utc>>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    lens: Option<String>,
    bit_depth: Option<i64>,
    color_space: Option<String>,
    exposure: Map<String, JsonValue>,
    gps_present: bool,
    capture_time_assumption: Option<&'static str>,
}

struct DecodeFacts {
    source_format: String,
    decoder: &'static str,
    proxy_provenance: PhotoProxyProvenance,
    bit_depth: Option<i64>,
    color_space: Option<String>,
    icc_profile_name: Option<String>,
    icc_profile_sha256: Option<String>,
    icc_profile: Option<Vec<u8>>,
}

pub fn is_supported_photo_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            PHOTO_EXTENSIONS
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

pub fn photo_support_matrix() -> Vec<PhotoCapability> {
    let image_io_formats = macos_imageio_formats(&CancellationToken::default()).unwrap_or_default();
    PHOTO_EXTENSIONS
        .iter()
        .map(|extension| {
            if IMAGE_RS_EXTENSIONS.contains(extension) {
                return PhotoCapability {
                    extension,
                    decoder: "image-rs",
                    available: true,
                    reason: "bundled Rust decoder performs a full source decode".to_owned(),
                };
            }
            let available = image_io_formats.contains(*extension);
            PhotoCapability {
                extension,
                decoder: "macos-imageio",
                available,
                reason: if available {
                    "installed macOS ImageIO reports a full render path".to_owned()
                } else if cfg!(target_os = "macos") {
                    format!("installed macOS ImageIO does not advertise .{extension}")
                } else {
                    "requires the macOS ImageIO decoder; this platform has no enabled full decoder"
                        .to_owned()
                },
            }
        })
        .collect()
}

/// Decode one photo through its enabled full decoder. The macOS ImageIO path spawns sips
/// subprocesses, so the pipeline cancellation token is threaded through: cancelling ingest
/// kills a running sips child instead of hanging on it.
pub fn decode_photo(path: &Path, cancellation: &CancellationToken) -> anyhow::Result<DecodedPhoto> {
    let extension = extension(path)?;
    if IMAGE_RS_EXTENSIONS.contains(&extension.as_str()) {
        decode_with_image_rs(path, &extension)
    } else if IMAGE_IO_EXTENSIONS.contains(&extension.as_str()) {
        decode_with_macos_imageio(path, &extension, cancellation)
    } else {
        bail!("unsupported photo extension .{extension}")
    }
}

pub fn write_jpeg_derivative(
    image: &DynamicImage,
    output: &Path,
    max_dimension: u32,
    quality: u8,
    icc_profile: Option<&[u8]>,
) -> anyhow::Result<Derivative> {
    ensure!(
        max_dimension > 0,
        "derivative maximum dimension must be positive"
    );
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let resized = image.thumbnail(max_dimension, max_dimension).to_rgb8();
    let (width, height) = resized.dimensions();
    let temporary = output.with_extension("jpg.partial");
    let mut encoder = JpegEncoder::new_with_quality(
        File::create(&temporary)
            .with_context(|| format!("failed to create derivative {}", temporary.display()))?,
        quality,
    );
    if let Some(profile) = icc_profile {
        encoder
            .set_icc_profile(profile.to_vec())
            .context("JPEG encoder rejected the source ICC profile")?;
    }
    encoder
        .encode_image(&resized)
        .with_context(|| format!("failed to encode derivative {}", output.display()))?;
    std::fs::rename(&temporary, output)
        .with_context(|| format!("failed to publish derivative {}", output.display()))?;
    Ok(Derivative {
        path: output.to_path_buf(),
        width,
        height,
        sha256: crate::sha256_file(output)?,
    })
}

fn decode_with_image_rs(path: &Path, extension: &str) -> anyhow::Result<DecodedPhoto> {
    let reader = ImageReader::open(path)
        .with_context(|| format!("failed to open photo {}", path.display()))?
        .with_guessed_format()
        .context("failed to identify photo container")?;
    let mut decoder = reader
        .into_decoder()
        .with_context(|| format!("failed to initialize decoder for {}", path.display()))?;
    let original_color_type = decoder.original_color_type();
    let bit_depth = Some(i64::from(
        original_color_type.bits_per_pixel() / u16::from(original_color_type.channel_count()),
    ));
    let icc_profile = decoder
        .icc_profile()
        .context("failed to read ICC profile")?;
    let exif_bytes = decoder
        .exif_metadata()
        .context("failed to read EXIF block")?;
    let extracted = exif_bytes
        .and_then(|bytes| ExifReader::new().read_raw(bytes).ok())
        .map(|exif| extract_exif(&exif))
        .unwrap_or_default();
    let orientation = extracted
        .orientation
        .and_then(Orientation::from_exif)
        .unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)
        .with_context(|| format!("failed to decode photo {}", path.display()))?;
    image.apply_orientation(orientation);
    let color_space = extracted.color_space.clone().or_else(|| {
        icc_profile
            .as_ref()
            .map(|_| "ICC profile embedded".to_owned())
    });
    let icc_profile_sha256 = icc_profile.as_deref().map(sha256_bytes);
    Ok(decoded_photo(
        image,
        extracted,
        DecodeFacts {
            source_format: extension.to_owned(),
            decoder: "image-rs",
            proxy_provenance: PhotoProxyProvenance::DecodedOriginal,
            bit_depth,
            color_space,
            icc_profile_name: None,
            icc_profile_sha256,
            icc_profile,
        },
    ))
}

#[cfg(target_os = "macos")]
fn decode_with_macos_imageio(
    path: &Path,
    extension: &str,
    cancellation: &CancellationToken,
) -> anyhow::Result<DecodedPhoto> {
    let formats = macos_imageio_formats(cancellation)?;
    ensure!(
        formats.contains(extension),
        "installed macOS ImageIO does not advertise a full decoder for .{extension}"
    );
    let mut extracted = read_container_exif(path).unwrap_or_default();
    let properties = sips_properties(path, cancellation)?;
    extracted.camera_make = extracted
        .camera_make
        .or_else(|| properties.get("make").cloned());
    extracted.camera_model = extracted
        .camera_model
        .or_else(|| properties.get("model").cloned());
    if extracted.captured_at.is_none() {
        if let Some(creation) = properties.get("creation") {
            extracted.captured_at = parse_property_time(creation);
            if extracted.captured_at.is_some() {
                extracted.capture_time_assumption = Some("macos_imageio_creation");
            }
        }
    }
    let temporary = tempfile::tempdir().context("failed to create ImageIO render directory")?;
    let rendered = temporary.path().join("rendered.jpg");
    let mut command = Command::new("/usr/bin/sips");
    command
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "best"])
        .arg(path)
        .arg("--out")
        .arg(&rendered);
    let output = run_sips_with_control(&mut command, cancellation)
        .context("failed to run macOS ImageIO through sips")?;
    ensure!(
        output.status.success(),
        "macOS ImageIO could not fully render .{extension}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let reader = ImageReader::open(&rendered)?.with_guessed_format()?;
    let mut decoder = reader.into_decoder().with_context(|| {
        format!("macOS ImageIO rendered .{extension}, but its output could not be decoded")
    })?;
    let icc_profile = decoder.icc_profile()?;
    let rendered_orientation = decoder
        .exif_metadata()?
        .and_then(|bytes| ExifReader::new().read_raw(bytes).ok())
        .and_then(|exif| extract_exif(&exif).orientation);
    extracted.orientation = rendered_orientation.or(extracted.orientation);
    let orientation = extracted
        .orientation
        .and_then(Orientation::from_exif)
        .unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let bit_depth = properties
        .get("bitsPerSample")
        .and_then(|value| value.parse::<i64>().ok())
        .or(extracted.bit_depth);
    let color_space = properties
        .get("space")
        .cloned()
        .or(extracted.color_space.clone());
    let icc_profile_name = properties.get("profile").cloned();
    Ok(decoded_photo(
        image,
        extracted,
        DecodeFacts {
            source_format: extension.to_owned(),
            decoder: "macos-imageio",
            proxy_provenance: PhotoProxyProvenance::FullRender,
            bit_depth,
            color_space,
            icc_profile_name,
            icc_profile_sha256: icc_profile.as_deref().map(sha256_bytes),
            icc_profile,
        },
    ))
}

#[cfg(not(target_os = "macos"))]
fn decode_with_macos_imageio(
    _path: &Path,
    extension: &str,
    _cancellation: &CancellationToken,
) -> anyhow::Result<DecodedPhoto> {
    bail!(
        ".{extension} requires the macOS ImageIO decoder; this platform has no enabled full decoder"
    )
}

fn decoded_photo(
    image: DynamicImage,
    extracted: ExtractedExif,
    facts: DecodeFacts,
) -> DecodedPhoto {
    let metadata_json = json!({
        "gps_policy": "presence_only_no_coordinates",
        "capture_time_assumption": extracted.capture_time_assumption,
        "orientation_normalized_in_derivatives": true,
    })
    .to_string();
    DecodedPhoto {
        image,
        source_format: facts.source_format,
        decoder: facts.decoder.to_owned(),
        proxy_provenance: facts.proxy_provenance,
        orientation: extracted.orientation.map(i64::from),
        orientation_applied: true,
        captured_at: extracted.captured_at,
        camera_make: extracted.camera_make,
        camera_model: extracted.camera_model,
        lens: extracted.lens,
        bit_depth: facts.bit_depth.or(extracted.bit_depth),
        color_space: facts.color_space,
        icc_profile_name: facts.icc_profile_name,
        icc_profile_sha256: facts.icc_profile_sha256,
        icc_profile: facts.icc_profile,
        exposure_json: JsonValue::Object(extracted.exposure).to_string(),
        gps_present: extracted.gps_present,
        metadata_json,
    }
}

#[cfg(target_os = "macos")]
fn read_container_exif(path: &Path) -> anyhow::Result<ExtractedExif> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let exif = ExifReader::new()
        .read_from_container(&mut reader)
        .context("container has no readable EXIF metadata")?;
    Ok(extract_exif(&exif))
}

fn extract_exif(exif: &exif::Exif) -> ExtractedExif {
    let orientation = field_uint(exif, Tag::Orientation).and_then(|value| u8::try_from(value).ok());
    let (captured_at, capture_time_assumption) = capture_time(exif);
    let mut exposure = Map::new();
    for (key, tag) in [
        ("iso", Tag::PhotographicSensitivity),
        ("f_number", Tag::FNumber),
        ("exposure_time", Tag::ExposureTime),
        ("focal_length", Tag::FocalLength),
        ("exposure_bias", Tag::ExposureBiasValue),
    ] {
        if let Some(value) = field_display(exif, tag) {
            exposure.insert(key.to_owned(), JsonValue::String(value));
        }
    }
    let gps_present = [
        Tag::GPSLatitude,
        Tag::GPSLongitude,
        Tag::GPSAltitude,
        Tag::GPSDateStamp,
    ]
    .iter()
    .any(|tag| exif.get_field(*tag, In::PRIMARY).is_some());
    ExtractedExif {
        orientation,
        captured_at,
        camera_make: field_ascii(exif, Tag::Make),
        camera_model: field_ascii(exif, Tag::Model),
        lens: field_ascii(exif, Tag::LensModel),
        bit_depth: field_uint(exif, Tag::BitsPerSample).map(i64::from),
        color_space: field_display(exif, Tag::ColorSpace),
        exposure,
        gps_present,
        capture_time_assumption,
    }
}

fn capture_time(exif: &exif::Exif) -> (Option<DateTime<Utc>>, Option<&'static str>) {
    let Some(timestamp) = field_ascii(exif, Tag::DateTimeOriginal) else {
        return (None, None);
    };
    if let Some(offset) = field_ascii(exif, Tag::OffsetTimeOriginal) {
        let value = format!("{timestamp} {offset}");
        if let Ok(parsed) = DateTime::parse_from_str(&value, "%Y:%m:%d %H:%M:%S %:z") {
            return (Some(parsed.with_timezone(&Utc)), Some("exif_offset"));
        }
    }
    let parsed = NaiveDateTime::parse_from_str(&timestamp, "%Y:%m:%d %H:%M:%S")
        .ok()
        .map(|value| value.and_utc());
    (parsed, parsed.map(|_| "utc_when_exif_offset_missing"))
}

#[cfg(target_os = "macos")]
fn parse_property_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y:%m:%d %H:%M:%S")
                .ok()
                .map(|parsed| parsed.and_utc())
        })
}

fn field_uint(exif: &exif::Exif, tag: Tag) -> Option<u32> {
    exif.get_field(tag, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
}

fn field_ascii(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    let Value::Ascii(values) = &field.value else {
        return None;
    };
    values.first().and_then(|value| {
        let cleaned = String::from_utf8_lossy(value)
            .trim_matches(|character: char| character == '\0' || character.is_whitespace())
            .to_owned();
        (!cleaned.is_empty()).then_some(cleaned)
    })
}

fn field_display(exif: &exif::Exif, tag: Tag) -> Option<String> {
    exif.get_field(tag, In::PRIMARY).map(|field| {
        field
            .display_value()
            .with_unit(exif)
            .to_string()
            .trim()
            .to_owned()
    })
}

fn extension(path: &Path) -> anyhow::Result<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .with_context(|| format!("photo has no UTF-8 extension: {}", path.display()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Run one sips subprocess with pipeline cancellation and a hard 120 s timeout, mirroring the
/// ffmpeg `run_progress` pattern: spawn in its own process group, drain pipes on side threads,
/// poll `try_wait`, and kill the group on cancellation or timeout. Non-Unix builds kill the
/// child directly.
#[cfg(target_os = "macos")]
fn run_sips_with_control(
    command: &mut Command,
    cancellation: &CancellationToken,
) -> anyhow::Result<std::process::Output> {
    run_sips_with_timeout(command, cancellation, SIPS_TIMEOUT)
}

#[cfg(target_os = "macos")]
fn run_sips_with_timeout(
    command: &mut Command,
    cancellation: &CancellationToken,
    timeout: Duration,
) -> anyhow::Result<std::process::Output> {
    ensure!(
        !cancellation.is_cancelled(),
        "sips was cancelled before it started"
    );
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.process_group(0);
    let mut child = command
        .spawn()
        .context("failed to start sips (macOS ImageIO)")?;
    let mut stdout_pipe = child.stdout.take().context("sips stdout pipe missing")?;
    let mut stderr_pipe = child.stderr.take().context("sips stderr pipe missing")?;
    let stdout_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut bytes);
        bytes
    });
    let stderr_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let status = loop {
        if cancellation.is_cancelled() {
            kill_sips(&mut child);
            child.wait()?;
            bail!("sips invocation was cancelled");
        }
        if started.elapsed() >= timeout {
            kill_sips(&mut child);
            child.wait()?;
            bail!("sips invocation timed out after {} s", timeout.as_secs());
        }
        match child.try_wait()? {
            Some(status) => break status,
            None => thread::sleep(SIPS_POLL_INTERVAL),
        }
    };
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Kill the sips child's whole process group (the child was spawned with `process_group(0)`);
/// the non-Unix branch kills the child directly.
#[cfg(target_os = "macos")]
fn kill_sips(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let process_group = -(child.id() as libc::pid_t);
        // SAFETY: `kill` targets the process group created immediately before spawn.
        let _ = unsafe { libc::kill(process_group, libc::SIGKILL) };
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[cfg(target_os = "macos")]
fn macos_imageio_formats(cancellation: &CancellationToken) -> anyhow::Result<BTreeSet<String>> {
    if let Some(formats) = MACOS_IMAGEIO_FORMATS.get() {
        return Ok(formats.clone());
    }
    let mut command = Command::new("/usr/bin/sips");
    command.arg("--formats");
    let output = run_sips_with_control(&mut command, cancellation)
        .context("failed to query macOS ImageIO formats")?;
    ensure!(
        output.status.success(),
        "macOS ImageIO format query failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let formats = parse_sips_formats(&String::from_utf8_lossy(&output.stdout));
    let _ = MACOS_IMAGEIO_FORMATS.set(formats.clone());
    Ok(formats)
}

#[cfg(not(target_os = "macos"))]
fn macos_imageio_formats(_cancellation: &CancellationToken) -> anyhow::Result<BTreeSet<String>> {
    Ok(BTreeSet::new())
}

#[cfg(any(target_os = "macos", test))]
fn parse_sips_formats(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter(|extension| *extension != "--")
        .map(str::to_ascii_lowercase)
        .collect()
}

#[cfg(target_os = "macos")]
fn sips_properties(
    path: &Path,
    cancellation: &CancellationToken,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let mut command = Command::new("/usr/bin/sips");
    command.args([
        "-g",
        "bitsPerSample",
        "-g",
        "space",
        "-g",
        "profile",
        "-g",
        "make",
        "-g",
        "model",
        "-g",
        "creation",
    ]);
    command.arg(path);
    let output = run_sips_with_control(&mut command, cancellation)
        .context("failed to read macOS ImageIO properties")?;
    ensure!(
        output.status.success(),
        "macOS ImageIO property query failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().split_once(':'))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .filter(|(_, value)| value != "<nil>" && value != "(null)")
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer, Rgb};
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Matrix {
        photos: Vec<MatrixPhoto>,
    }

    #[derive(Deserialize)]
    struct MatrixPhoto {
        extensions: Vec<String>,
    }

    #[test]
    fn parses_imageio_capability_output_without_claiming_missing_formats() {
        let formats = parse_sips_formats(
            "Supported Formats:\npublic.heic heic Writable\ncom.canon.cr3-raw-image cr3\npublic.avci --\n",
        );
        assert!(formats.contains("heic"));
        assert!(formats.contains("cr3"));
        assert!(!formats.contains("--"));
        assert!(!formats.contains("nef"));
    }

    #[test]
    fn every_task_format_has_an_explicit_capability_result() {
        let matrix = photo_support_matrix();
        assert_eq!(matrix.len(), PHOTO_EXTENSIONS.len());
        for extension in PHOTO_EXTENSIONS {
            let capability = matrix
                .iter()
                .find(|capability| capability.extension == *extension)
                .unwrap();
            assert!(!capability.decoder.is_empty());
            assert!(!capability.reason.is_empty());
        }
    }

    #[test]
    fn checked_in_support_matrix_and_imageio_capture_cover_every_photo_extension() {
        let matrix: Matrix = serde_json::from_str(include_str!(
            "../../../fixtures/source-formats/support-matrix.json"
        ))
        .unwrap();
        let declared = matrix
            .photos
            .into_iter()
            .flat_map(|photo| photo.extensions)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            declared,
            PHOTO_EXTENSIONS
                .iter()
                .map(|value| (*value).to_owned())
                .collect()
        );

        let captured = parse_sips_formats(include_str!(
            "../../../fixtures/source-formats/macos-imageio-task-016.txt"
        ));
        for extension in IMAGE_IO_EXTENSIONS {
            assert!(captured.contains(*extension), "missing .{extension}");
        }
    }

    #[test]
    fn jpeg_derivative_is_byte_deterministic() {
        let temporary = tempfile::tempdir().unwrap();
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(32, 24, |x, y| {
            Rgb([(x * 7) as u8, (y * 9) as u8, ((x + y) * 3) as u8])
        }));
        let first =
            write_jpeg_derivative(&image, &temporary.path().join("first.jpg"), 16, 92, None)
                .unwrap();
        let second =
            write_jpeg_derivative(&image, &temporary.path().join("second.jpg"), 16, 92, None)
                .unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!((first.width, first.height), (16, 12));
    }

    #[test]
    fn applies_all_exif_orientations_to_derivative_pixels() {
        let original = DynamicImage::ImageRgb8(ImageBuffer::from_fn(3, 2, |x, y| {
            Rgb([(x * 60) as u8, (y * 120) as u8, ((x + y) * 3) as u8])
        }));
        for value in 1..=8 {
            let orientation = Orientation::from_exif(value).unwrap();
            let mut image = original.clone();
            image.apply_orientation(orientation);
            if matches!(value, 5..=8) {
                assert_eq!(image.dimensions(), (2, 3));
            } else {
                assert_eq!(image.dimensions(), (3, 2));
            }
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn sips_helper_times_out_long_running_children() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let started = Instant::now();
        let result = run_sips_with_timeout(
            &mut command,
            &CancellationToken::default(),
            Duration::from_secs(1),
        );
        let elapsed = started.elapsed();
        let error = result.unwrap_err().to_string();
        assert!(error.contains("timed out"), "unexpected error: {error}");
        assert!(elapsed >= Duration::from_secs(1));
        assert!(elapsed < Duration::from_secs(30), "sleep was not killed");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn sips_helper_cancelled_before_spawn_never_starts_a_child() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let error = run_sips_with_control(&mut command, &cancellation)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cancelled"), "unexpected error: {error}");
    }
}
