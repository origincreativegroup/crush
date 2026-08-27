//! Task 0 feasibility spike. Throwaway and intentionally outside the product workspace.

use anyhow::{bail, ensure, Context};
use ort::{
    ep::{self, ExecutionProvider},
    logging::{LogLevel, LoggerFunction},
    session::Session,
    value::Tensor,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::Instant,
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const CLIP_RUNS: usize = 10;
const WHISPER_SECONDS: usize = 10;
const SAMPLE_RATE: usize = 16_000;

fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("spike directory should have a repository parent")?
        .to_path_buf();

    let total = Instant::now();
    run_ffmpeg(&root.join("sidecars/ffmpeg"))?;
    run_clip_coreml(&root.join("models/clip-vision-vit-b-32-fixed.onnx"))?;
    run_whisper_metal(
        &root.join("models/ggml-base.en.bin"),
        &root.join("fixtures/spike-jfk.wav"),
    )?;
    println!(
        "SPIKE_OK total_ms={:.2}",
        total.elapsed().as_secs_f64() * 1_000.0
    );
    Ok(())
}

fn run_ffmpeg(binary: &Path) -> anyhow::Result<()> {
    ensure!(
        binary.is_file(),
        "bundled ffmpeg missing at {}",
        binary.display()
    );

    let started = Instant::now();
    let output = Command::new(binary)
        .arg("-version")
        .output()
        .with_context(|| format!("failed to spawn {}", binary.display()))?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;

    ensure!(output.status.success(), "bundled ffmpeg -version failed");
    let version = String::from_utf8(output.stdout).context("ffmpeg output was not UTF-8")?;
    let configuration = version
        .lines()
        .find(|line| line.starts_with("configuration:"))
        .context("ffmpeg did not report its configuration")?;
    ensure!(
        configuration.contains("--enable-static"),
        "ffmpeg is not a static-library build"
    );
    ensure!(
        !configuration.contains("--enable-gpl"),
        "ffmpeg build unexpectedly enables GPL code"
    );
    ensure!(
        !configuration.contains("--enable-nonfree"),
        "ffmpeg build unexpectedly enables nonfree code"
    );

    let version_line = version.lines().next().unwrap_or("unknown ffmpeg");
    println!("FFMPEG_OK spawn_ms={elapsed_ms:.2} version={version_line}");
    println!("FFMPEG_CONFIG={configuration}");
    Ok(())
}

fn run_clip_coreml(model: &Path) -> anyhow::Result<()> {
    ensure!(model.is_file(), "CLIP ONNX missing at {}", model.display());

    let coreml = ep::CoreML::default();
    ensure!(
        coreml.is_available()?,
        "ONNX Runtime was not built with CoreML support"
    );

    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_logs = Arc::clone(&logs);
    let logger: LoggerFunction = Arc::new(move |level, _category, _id, _location, message| {
        if message.contains("CoreML") || level >= LogLevel::Warning {
            eprintln!("ORT_{level:?}: {message}");
        }
        captured_logs
            .lock()
            .expect("ORT log lock poisoned")
            .push(message.to_owned());
    });

    let profile_prefix = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("coreml-profile.json");
    std::fs::create_dir_all(
        profile_prefix
            .parent()
            .context("profile path had no parent")?,
    )?;

    let session_started = Instant::now();
    let builder = Session::builder()?;
    let builder = builder
        .with_execution_providers([ep::CoreML::default()
            .with_compute_units(ep::coreml::ComputeUnits::All)
            .with_model_format(ep::coreml::ModelFormat::MLProgram)
            .with_static_input_shapes(true)
            .with_profile_compute_plan(true)
            .build()
            .error_on_failure()])
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let builder = builder
        .with_logger(logger)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let builder = builder
        .with_log_level(LogLevel::Verbose)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let builder = builder
        .with_log_verbosity(1)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mut session = builder
        .with_profiling(&profile_prefix)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?
        .commit_from_file(model)
        .with_context(|| format!("failed to load CLIP model {}", model.display()))?;
    let session_ms = session_started.elapsed().as_secs_f64() * 1_000.0;

    println!("CLIP_INPUTS={:?}", session.inputs());
    println!("CLIP_OUTPUTS={:?}", session.outputs());

    let input = Tensor::from_array(([1_usize, 3, 224, 224], vec![0.0_f32; 3 * 224 * 224]))?;
    let warmup = session.run(ort::inputs![&input])?;
    validate_clip_output(&warmup[0])?;
    drop(warmup);

    let inference_started = Instant::now();
    for _ in 0..CLIP_RUNS {
        let outputs = session.run(ort::inputs![&input])?;
        validate_clip_output(&outputs[0])?;
    }
    let inference_ms = inference_started.elapsed().as_secs_f64() * 1_000.0;
    let profile_file = session.end_profiling()?;
    let profile = std::fs::read_to_string(&profile_file)
        .with_context(|| format!("failed to read ORT profile {profile_file}"))?;
    let coreml_events = profile.matches("CoreMLExecutionProvider").count();
    ensure!(
        coreml_events > 0,
        "CoreML registered but the profiler found no CoreML-executed nodes; refusing CPU fallback"
    );
    ensure!(
        !profile.contains("CPUExecutionProvider"),
        "ONNX Runtime profile contains CPU-executed nodes; refusing partial CPU fallback"
    );

    let coreml_log_lines = logs
        .lock()
        .expect("ORT log lock poisoned")
        .iter()
        .filter(|line| line.contains("CoreML"))
        .count();
    println!(
        "COREML_ACTIVE=true profile_events={coreml_events} coreml_log_lines={coreml_log_lines} session_ms={session_ms:.2}"
    );
    println!(
        "CLIP_OK runs={CLIP_RUNS} total_ms={inference_ms:.2} mean_ms={:.2}",
        inference_ms / CLIP_RUNS as f64
    );
    println!("COREML_PROFILE={profile_file}");
    Ok(())
}

fn validate_clip_output(output: &ort::value::DynValue) -> anyhow::Result<()> {
    let (shape, values) = output.try_extract_tensor::<f32>()?;
    ensure!(
        !values.is_empty(),
        "CLIP returned an empty tensor with shape {shape:?}"
    );
    ensure!(
        values.iter().all(|value| value.is_finite()),
        "CLIP returned non-finite values"
    );
    Ok(())
}

fn run_whisper_metal(model: &Path, wav: &Path) -> anyhow::Result<()> {
    ensure!(
        model.is_file(),
        "Whisper model missing at {}",
        model.display()
    );
    ensure!(
        wav.is_file(),
        "Whisper fixture missing at {}",
        wav.display()
    );

    let mut reader = hound::WavReader::open(wav)
        .with_context(|| format!("failed to open WAV fixture {}", wav.display()))?;
    let spec = reader.spec();
    ensure!(
        spec.sample_rate as usize == SAMPLE_RATE,
        "WAV must be 16 kHz"
    );
    ensure!(spec.channels == 1, "WAV must be mono");
    ensure!(spec.bits_per_sample == 16, "WAV must contain 16-bit PCM");

    let samples = reader
        .samples::<i16>()
        .take(SAMPLE_RATE * WHISPER_SECONDS)
        .collect::<Result<Vec<_>, _>>()?;
    ensure!(
        samples.len() == SAMPLE_RATE * WHISPER_SECONDS,
        "WAV is shorter than 10 seconds"
    );
    let mut audio = vec![0.0_f32; samples.len()];
    whisper_rs::convert_integer_to_float_audio(&samples, &mut audio)?;

    let mut context_params = WhisperContextParameters::default();
    context_params.use_gpu(true).flash_attn(true);
    println!(
        "WHISPER_METAL_REQUESTED={} whisper_cpp_version={}",
        context_params.use_gpu,
        whisper_rs::WHISPER_CPP_VERSION
    );

    let context_started = Instant::now();
    let context = WhisperContext::new_with_params(
        model.to_str().context("Whisper model path was not UTF-8")?,
        context_params,
    )?;
    let context_ms = context_started.elapsed().as_secs_f64() * 1_000.0;
    let mut state = context.create_state()?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism()
        .map(|count| count.get().saturating_sub(2).max(1))
        .unwrap_or(1);
    params.set_n_threads(threads as i32);
    params.set_language(Some("en"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    let inference_started = Instant::now();
    state.full(params, &audio)?;
    let inference_ms = inference_started.elapsed().as_secs_f64() * 1_000.0;
    let transcript = state
        .as_iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    if transcript.trim().is_empty() {
        bail!("Whisper completed without returning a transcript");
    }

    println!("WHISPER_TRANSCRIPT={}", transcript.trim());
    println!(
        "WHISPER_OK audio_s={WHISPER_SECONDS} context_ms={context_ms:.2} inference_ms={inference_ms:.2} threads={threads}"
    );
    Ok(())
}
