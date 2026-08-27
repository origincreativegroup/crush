//! Task 0 feasibility spike. Throwaway. Not a workspace member.
//! Prove on John's Mac, printing milliseconds for each:
//!   1. ort loads clip-image.onnx with CoreML execution provider ACTIVE (not silently CPU)
//!   2. whisper-rs transcribes fixtures/spike-10s.wav with Metal
//!   3. a static ffmpeg binary in ../sidecars/ runs `-version`
//! Exit non-zero if any step fails. See spike/README.md.
fn main() -> anyhow::Result<()> {
    println!("spike: fill in per spike/README.md");
    Ok(())
}
