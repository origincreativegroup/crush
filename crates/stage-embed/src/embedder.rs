//! ONNX Runtime-backed CLIP image and text embeddings.

use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{ensure, Context};
use ort::{
    ep::{self, ExecutionProvider},
    logging::{LogLevel, LoggerFunction},
    session::Session,
    value::Tensor as OrtTensor,
};

use crate::{
    preprocess::{Tensor, TENSOR_LEN},
    tokenizer::{ClipTokenizer, CONTEXT_LENGTH},
};

const IMAGE_MODEL: &str = "clip-image.onnx";
const TEXT_MODEL: &str = "clip-text.onnx";
const BPE_VOCAB: &str = "bpe_simple_vocab_16e6.txt.gz";
pub const EMBEDDING_DIM: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPreference {
    Cpu,
    CoreMl,
}

impl ProviderPreference {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "cpu" => Ok(Self::Cpu),
            "coreml" => Ok(Self::CoreMl),
            _ => anyhow::bail!("unsupported embedding provider {value:?}; expected coreml or cpu"),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CoreMl => "coreml",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActiveProvider {
    Cpu,
    CoreMl,
}

impl ActiveProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::CoreMl => "coreml",
        }
    }
}

struct TrackedSession {
    session: Session,
    registered: ActiveProvider,
    active: BTreeSet<ActiveProvider>,
    profile_pending: bool,
    logs: Arc<Mutex<Vec<String>>>,
}

impl TrackedSession {
    fn verify_after_run(&mut self) -> anyhow::Result<()> {
        if !self.profile_pending {
            return Ok(());
        }
        let profile_path = self.session.end_profiling()?;
        self.profile_pending = false;
        let profile = std::fs::read_to_string(&profile_path)
            .with_context(|| format!("failed to read ONNX Runtime profile {profile_path}"))?;
        if profile.contains("CoreMLExecutionProvider") {
            self.active.insert(ActiveProvider::CoreMl);
        }
        if profile.contains("CPUExecutionProvider") {
            self.active.insert(ActiveProvider::Cpu);
        }
        if self.active.is_empty() {
            self.active.insert(ActiveProvider::Cpu);
        }
        let _ = std::fs::remove_file(profile_path);
        Ok(())
    }
}

pub struct Embedder {
    image: TrackedSession,
    text: TrackedSession,
    tokenizer: ClipTokenizer,
    requested: ProviderPreference,
    warnings: Vec<String>,
}

impl Embedder {
    pub fn new(
        models_dir: impl AsRef<Path>,
        provider: ProviderPreference,
        threads: usize,
    ) -> anyhow::Result<Self> {
        let models_dir = models_dir.as_ref();
        for name in [IMAGE_MODEL, TEXT_MODEL, BPE_VOCAB] {
            ensure!(
                models_dir.join(name).is_file(),
                "required embedding model is missing: {}",
                models_dir.join(name).display()
            );
        }
        let tokenizer = ClipTokenizer::from_gzip(models_dir.join(BPE_VOCAB))?;
        let resolved_threads = if threads == 0 {
            std::thread::available_parallelism()
                .map(|count| count.get().saturating_sub(2).max(1))
                .unwrap_or(1)
        } else {
            threads.max(1)
        };

        let mut warnings = Vec::new();
        let coreml_available = provider == ProviderPreference::CoreMl
            && ep::CoreML::default().is_available().unwrap_or(false);
        if provider == ProviderPreference::CoreMl && !coreml_available {
            warnings
                .push("ONNX Runtime reports CoreML unavailable on this host; using CPU".to_owned());
        }
        let requested_session_provider = if coreml_available {
            ActiveProvider::CoreMl
        } else {
            ActiveProvider::Cpu
        };
        let cache_root = models_dir.join("coreml-cache");
        let image_model = if coreml_available {
            coreml_keyed_model(models_dir, IMAGE_MODEL, &cache_root)?
        } else {
            models_dir.join(IMAGE_MODEL)
        };
        let text_model = if coreml_available {
            coreml_keyed_model(models_dir, TEXT_MODEL, &cache_root)?
        } else {
            models_dir.join(TEXT_MODEL)
        };
        let image = build_with_fallback(
            &image_model,
            &cache_root.join("image"),
            "image",
            requested_session_provider,
            resolved_threads,
            &mut warnings,
        )?;
        let text = build_with_fallback(
            &text_model,
            &cache_root.join("text"),
            "text",
            requested_session_provider,
            resolved_threads,
            &mut warnings,
        )?;
        for warning in &warnings {
            tracing::warn!(
                requested = provider.as_str(),
                reason = warning,
                "embedding provider fallback"
            );
        }
        Ok(Self {
            image,
            text,
            tokenizer,
            requested: provider,
            warnings,
        })
    }

    pub const fn requested_provider(&self) -> ProviderPreference {
        self.requested
    }

    /// The providers observed in ONNX Runtime profiles. Before the first run, this reports the
    /// successfully registered provider so callers can still show initialization state.
    pub fn active_providers(&self) -> Vec<ActiveProvider> {
        let mut providers = BTreeSet::new();
        for tracked in [&self.image, &self.text] {
            if tracked.active.is_empty() {
                providers.insert(tracked.registered);
            } else {
                providers.extend(tracked.active.iter().copied());
            }
        }
        providers.into_iter().collect()
    }

    pub fn active_provider(&self) -> ActiveProvider {
        if self.active_providers().contains(&ActiveProvider::CoreMl) {
            ActiveProvider::CoreMl
        } else {
            ActiveProvider::Cpu
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn tokenize(&mut self, text: &str) -> anyhow::Result<[i64; CONTEXT_LENGTH]> {
        self.tokenizer.encode(text)
    }

    pub fn embed_image(&mut self, input: &Tensor) -> anyhow::Result<[f32; EMBEDDING_DIM]> {
        ensure!(
            input.values().len() == TENSOR_LEN,
            "CLIP image tensor has wrong length"
        );
        let input = OrtTensor::from_array(([1_usize, 3, 224, 224], input.values().to_vec()))?;
        let values = {
            let outputs = self.image.session.run(ort::inputs![&input])?;
            extract_embedding(&outputs[0])?
        };
        self.image.verify_after_run()?;
        self.record_runtime_fallback("image");
        normalize(values)
    }

    pub fn embed_text(&mut self, text: &str) -> anyhow::Result<[f32; EMBEDDING_DIM]> {
        let ids = self.tokenizer.encode(text)?;
        let input = OrtTensor::from_array(([1_usize, CONTEXT_LENGTH], ids.to_vec()))?;
        let values = {
            let outputs = self.text.session.run(ort::inputs![&input])?;
            extract_embedding(&outputs[0])?
        };
        self.text.verify_after_run()?;
        self.record_runtime_fallback("text");
        normalize(values)
    }

    fn record_runtime_fallback(&mut self, kind: &str) {
        let tracked = if kind == "image" {
            &self.image
        } else {
            &self.text
        };
        if self.requested == ProviderPreference::CoreMl
            && !tracked.active.contains(&ActiveProvider::CoreMl)
        {
            let log_reason = tracked
                .logs
                .lock()
                .ok()
                .and_then(|logs| {
                    logs.iter()
                        .rev()
                        .find(|line| line.contains("CoreML"))
                        .cloned()
                })
                .unwrap_or_else(|| "runtime profile contained no CoreML-executed nodes".to_owned());
            let warning = format!("{kind} session fell back to CPU: {log_reason}");
            if !self.warnings.contains(&warning) {
                tracing::warn!(
                    requested = "coreml",
                    active = "cpu",
                    reason = log_reason,
                    "embedding provider fallback"
                );
                self.warnings.push(warning);
            }
        }
    }
}

/// CoreML otherwise hashes a temporary partition URL, which changes every process. Append a
/// protobuf `metadata_props` entry to an otherwise byte-identical derived ONNX copy. ONNX Runtime
/// propagates `COREML_CACHE_KEY` to each partition, making compiled artifacts reusable across CLI
/// launches and test binaries.
fn coreml_keyed_model(models_dir: &Path, name: &str, cache_root: &Path) -> anyhow::Result<PathBuf> {
    let manifest = crush_core::models::bundled_manifest()?;
    let expected = manifest
        .files
        .get(name)
        .with_context(|| format!("bundled manifest has no {name}"))?;
    let source = models_dir.join(name);
    let suffix = metadata_property("COREML_CACHE_KEY", &expected.sha256);
    let derived_dir = cache_root.join("keyed-models");
    std::fs::create_dir_all(&derived_dir)?;
    let stem = name.strip_suffix(".onnx").unwrap_or(name);
    let destination = derived_dir.join(format!("{stem}-{}.onnx", expected.sha256));
    if has_expected_suffix(&destination, expected.bytes, &suffix)? {
        return Ok(destination);
    }

    let temporary = derived_dir.join(format!(".{stem}-{}.part", std::process::id()));
    let mut reader = File::open(&source)
        .with_context(|| format!("failed to open pinned ONNX model {}", source.display()))?;
    ensure!(
        reader.metadata()?.len() == expected.bytes,
        "pinned ONNX model {} has wrong size; run `crushctl models ensure`",
        source.display()
    );
    let mut writer = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    std::io::copy(&mut reader, &mut writer)?;
    writer.write_all(&suffix)?;
    writer.sync_all()?;
    drop(writer);
    std::fs::rename(&temporary, &destination)?;
    ensure!(
        has_expected_suffix(&destination, expected.bytes, &suffix)?,
        "failed to create deterministic CoreML ONNX cache input"
    );
    Ok(destination)
}

fn has_expected_suffix(path: &Path, source_bytes: u64, suffix: &[u8]) -> anyhow::Result<bool> {
    let Ok(mut file) = File::open(path) else {
        return Ok(false);
    };
    if file.metadata()?.len() != source_bytes + suffix.len() as u64 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-(suffix.len() as i64)))?;
    let mut found = vec![0_u8; suffix.len()];
    file.read_exact(&mut found)?;
    Ok(found == suffix)
}

fn metadata_property(key: &str, value: &str) -> Vec<u8> {
    let mut entry = Vec::with_capacity(key.len() + value.len() + 4);
    entry.push(0x0A); // StringStringEntryProto.key, field 1
    push_varint(&mut entry, key.len() as u64);
    entry.extend_from_slice(key.as_bytes());
    entry.push(0x12); // StringStringEntryProto.value, field 2
    push_varint(&mut entry, value.len() as u64);
    entry.extend_from_slice(value.as_bytes());

    let mut field = Vec::with_capacity(entry.len() + 2);
    field.push(0x72); // ModelProto.metadata_props, field 14
    push_varint(&mut field, entry.len() as u64);
    field.extend_from_slice(&entry);
    field
}

fn push_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn build_with_fallback(
    model: &Path,
    cache_dir: &Path,
    kind: &str,
    provider: ActiveProvider,
    threads: usize,
    warnings: &mut Vec<String>,
) -> anyhow::Result<TrackedSession> {
    tracing::info!(
        stage = "embed",
        model = kind,
        provider = provider.as_str(),
        "initializing ONNX embedding session"
    );
    match build_session(model, cache_dir, kind, provider, threads) {
        Ok(session) => {
            tracing::info!(
                stage = "embed",
                model = kind,
                provider = provider.as_str(),
                "ONNX embedding session ready"
            );
            Ok(session)
        }
        Err(error) if provider == ActiveProvider::CoreMl => {
            let reason = format!("{kind} CoreML session creation failed: {error}; using CPU");
            warnings.push(reason);
            build_session(model, cache_dir, kind, ActiveProvider::Cpu, threads)
        }
        Err(error) => Err(error),
    }
}

fn build_session(
    model: &Path,
    cache_dir: &Path,
    kind: &str,
    provider: ActiveProvider,
    threads: usize,
) -> anyhow::Result<TrackedSession> {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&logs);
    let logger: LoggerFunction = Arc::new(move |level, _category, _id, _location, message| {
        if level >= LogLevel::Warning || message.contains("CoreML") {
            captured
                .lock()
                .expect("ORT log lock poisoned")
                .push(message.to_owned());
        }
    });
    let mut builder = Session::builder()?;
    builder = builder
        .with_intra_threads(threads)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    builder = builder
        .with_logger(logger)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    builder = builder
        .with_log_level(LogLevel::Warning)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let profile_pending = provider == ActiveProvider::CoreMl;
    if profile_pending {
        std::fs::create_dir_all(cache_dir)?;
        let dispatch = ep::CoreML::default()
            .with_compute_units(ep::coreml::ComputeUnits::All)
            .with_model_format(ep::coreml::ModelFormat::MLProgram)
            .with_static_input_shapes(true)
            .with_model_cache_dir(cache_dir.display().to_string())
            .build()
            .error_on_failure();
        builder = builder
            .with_execution_providers([dispatch])
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        // Keep diagnostics deterministic and leave cache invalidation to COREML_CACHE_KEY.
        let profile_prefix = cache_dir.join(format!("{kind}-provider-profile.json"));
        builder = builder
            .with_profiling(profile_prefix)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }
    let session = builder
        .commit_from_file(model)
        .with_context(|| format!("failed to load CLIP {kind} model {}", model.display()))?;
    Ok(TrackedSession {
        session,
        registered: provider,
        active: if provider == ActiveProvider::Cpu {
            BTreeSet::from([ActiveProvider::Cpu])
        } else {
            BTreeSet::new()
        },
        profile_pending,
        logs,
    })
}

fn extract_embedding(output: &ort::value::DynValue) -> anyhow::Result<Vec<f32>> {
    let (shape, values) = output.try_extract_tensor::<f32>()?;
    ensure!(
        values.len() == EMBEDDING_DIM,
        "CLIP returned shape {shape:?} with {} values; expected {EMBEDDING_DIM}",
        values.len()
    );
    ensure!(
        values.iter().all(|value| value.is_finite()),
        "CLIP returned non-finite values"
    );
    Ok(values.to_vec())
}

fn normalize(values: Vec<f32>) -> anyhow::Result<[f32; EMBEDDING_DIM]> {
    let norm = values
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    ensure!(
        norm.is_finite() && norm > 1e-12,
        "CLIP returned a zero or invalid embedding norm"
    );
    let mut normalized = [0.0_f32; EMBEDDING_DIM];
    for (target, value) in normalized.iter_mut().zip(values) {
        *target = (f64::from(value) / norm) as f32;
    }
    let final_norm = normalized
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    ensure!(
        (final_norm - 1.0).abs() <= 1e-5,
        "normalized CLIP vector has norm {final_norm}"
    );
    Ok(normalized)
}
