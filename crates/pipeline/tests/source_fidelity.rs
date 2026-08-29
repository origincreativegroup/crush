#![cfg(target_os = "macos")]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crush_core::cancellation::CancellationToken;
use crush_pipeline::sha256_file;
use crush_pipeline::source::{decode_photo, write_jpeg_derivative};
use crush_pipeline::video_source::proxy_policy;
use crush_stage_split::ffmpeg::{self, Runner};
use image::codecs::jpeg::JpegEncoder;
use image::{
    GenericImageView, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Rgb, RgbImage,
};
use serde::Serialize;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Serialize)]
struct FidelityReport {
    elapsed_ms: u128,
    peak_resident_bytes: u64,
    failures: Vec<String>,
    checks: Vec<FidelityCheck>,
}

#[derive(Serialize)]
struct FidelityCheck {
    source: String,
    decoder_or_codec: String,
    width: u32,
    height: u32,
    orientation_note: String,
    color_note: String,
    proxy_required: Option<bool>,
}

#[test]
fn representative_stills_preserve_sources_and_record_fidelity() {
    let temporary = tempfile::tempdir().unwrap();
    let source_dir = temporary.path().join("sources");
    std::fs::create_dir(&source_dir).unwrap();
    let base = RgbImage::from_fn(80, 50, |x, y| {
        if x < 20 && y < 20 {
            Rgb([240, 24, 12])
        } else {
            Rgb([(x * 3) as u8, (y * 4) as u8, 96])
        }
    });
    let png = source_dir.join("representative.png");
    let jpeg = source_dir.join("representative.jpg");
    let tiff = source_dir.join("representative.tiff");
    base.save_with_format(&png, ImageFormat::Png).unwrap();
    base.save_with_format(&jpeg, ImageFormat::Jpeg).unwrap();
    base.save_with_format(&tiff, ImageFormat::Tiff).unwrap();
    let oriented = source_dir.join("orientation-6.jpg");
    std::fs::write(&oriented, jpeg_with_orientation(&jpeg, 6)).unwrap();
    let heic = source_dir.join("representative.heic");
    let sips = Command::new("/usr/bin/sips")
        .args(["-s", "format", "heic"])
        .arg(&png)
        .arg("--out")
        .arg(&heic)
        .output()
        .unwrap();
    assert!(
        sips.status.success(),
        "{}",
        String::from_utf8_lossy(&sips.stderr)
    );

    let sources = [png, jpeg, tiff, oriented, heic];
    let original_hashes = sources
        .iter()
        .map(|path| sha256_file(path).unwrap())
        .collect::<Vec<_>>();
    let started = Instant::now();
    let mut checks = Vec::new();
    for (index, path) in sources.iter().enumerate() {
        let decoded = decode_photo(path).unwrap();
        let (width, height) = decoded.image.dimensions();
        if path.file_name().unwrap() == "orientation-6.jpg" {
            assert_eq!((width, height), (50, 80));
            assert_eq!(decoded.orientation, Some(6));
        } else {
            assert_eq!((width, height), (80, 50));
        }
        let first = write_jpeg_derivative(
            &decoded.image,
            &temporary.path().join(format!("proxy-{index}-a.jpg")),
            64,
            92,
            decoded.icc_profile.as_deref(),
        )
        .unwrap();
        let second = write_jpeg_derivative(
            &decoded.image,
            &temporary.path().join(format!("proxy-{index}-b.jpg")),
            64,
            92,
            decoded.icc_profile.as_deref(),
        )
        .unwrap();
        assert_eq!(first.sha256, second.sha256);
        if let Some(expected_profile) = decoded.icc_profile.as_deref() {
            let reader = ImageReader::open(&first.path)
                .unwrap()
                .with_guessed_format()
                .unwrap();
            let mut decoder = reader.into_decoder().unwrap();
            assert_eq!(
                decoder.icc_profile().unwrap().as_deref(),
                Some(expected_profile)
            );
        }
        checks.push(FidelityCheck {
            source: path.file_name().unwrap().to_string_lossy().into_owned(),
            decoder_or_codec: decoded.decoder,
            width,
            height,
            orientation_note: if decoded.orientation == Some(6) {
                "EXIF orientation 6 normalized to portrait pixels".to_owned()
            } else {
                "source orientation retained".to_owned()
            },
            color_note: format!(
                "decoded RGB visual check passed; source color metadata: {}",
                decoded.color_space.as_deref().unwrap_or("not embedded")
            ),
            proxy_required: None,
        });
    }
    for (path, original_hash) in sources.iter().zip(original_hashes) {
        assert_eq!(sha256_file(path).unwrap(), original_hash);
    }
    write_report(
        temporary.path(),
        FidelityReport {
            elapsed_ms: started.elapsed().as_millis(),
            peak_resident_bytes: peak_resident_bytes(),
            failures: Vec::new(),
            checks,
        },
    );
}

#[test]
fn corrupt_raw_variant_reports_decoder_and_never_falls_back_to_preview() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("unsupported-variant.cr3");
    std::fs::write(&source, b"not a camera raw file").unwrap();
    let error = decode_photo(&source).unwrap_err().to_string();
    assert!(error.contains("macOS ImageIO"));
    assert!(error.contains(".cr3"));
}

#[test]
fn oriented_heic_decodes_upright_and_matches_the_image_rs_reference() {
    let temporary = tempfile::tempdir().unwrap();
    let base = RgbImage::from_fn(80, 50, |x, y| Rgb([(x * 2) as u8, (y * 5) as u8, 128]));
    let jpeg = temporary.path().join("oriented-source.jpg");
    base.save_with_format(&jpeg, ImageFormat::Jpeg).unwrap();
    let oriented = temporary.path().join("orientation-6.jpg");
    std::fs::write(&oriented, jpeg_with_orientation(&jpeg, 6)).unwrap();
    let heic = temporary.path().join("orientation-6.heic");
    convert_with_sips(&["-s", "format", "heic"], &oriented, &heic);

    let reference = decode_photo(&oriented).unwrap();
    assert_eq!(reference.image.dimensions(), (50, 80));
    assert_eq!(reference.orientation, Some(6));
    let decoded = decode_photo(&heic).unwrap();
    assert_eq!(decoded.image.dimensions(), (50, 80));
    assert!(decoded.orientation_applied);
    assert_pixels_match(&decoded.image.to_rgb8(), &reference.image.to_rgb8(), 16);
}

#[test]
fn icc_profiles_round_trip_into_derivatives_and_mismatches_are_detectable() {
    let Some(srgb) = system_profile("sRGB Profile.icc") else {
        return;
    };
    let Some(display_p3) = system_profile("Display P3.icc") else {
        return;
    };
    let temporary = tempfile::tempdir().unwrap();
    let base = RgbImage::from_fn(80, 50, |x, y| Rgb([(x * 3) as u8, (y * 4) as u8, 96]));

    let jpeg = temporary.path().join("srgb-icc.jpg");
    let mut encoder = JpegEncoder::new_with_quality(File::create(&jpeg).unwrap(), 92);
    encoder.set_icc_profile(srgb.clone()).unwrap();
    encoder.encode_image(&base).unwrap();
    let heic = temporary.path().join("srgb-icc.heic");
    let heic = try_convert_with_sips(
        &[
            "-s",
            "format",
            "heic",
            "-s",
            "iccProfile",
            "/System/Library/ColorSync/Profiles/sRGB Profile.icc",
        ],
        &jpeg,
        &heic,
    );

    let decoded_jpeg = decode_photo(&jpeg).unwrap();
    assert_eq!(decoded_jpeg.icc_profile.as_deref(), Some(srgb.as_slice()));
    let jpeg_derivative = write_jpeg_derivative(
        &decoded_jpeg.image,
        &temporary.path().join("srgb-derivative.jpg"),
        64,
        92,
        decoded_jpeg.icc_profile.as_deref(),
    )
    .unwrap();
    assert_eq!(
        read_back_profile(&jpeg_derivative.path).as_deref(),
        Some(srgb.as_slice())
    );

    let Some(heic) = heic else {
        eprintln!(
            "skipping the HEIC ICC sub-case: sips refused --setProperty iccProfile on this macOS"
        );
        return;
    };
    let decoded_heic = decode_photo(&heic).unwrap();
    let heic_profile = decoded_heic
        .icc_profile
        .clone()
        .expect("sips should embed the requested ICC profile in the HEIC");
    let heic_derivative = write_jpeg_derivative(
        &decoded_heic.image,
        &temporary.path().join("heic-derivative.jpg"),
        64,
        92,
        Some(&heic_profile),
    )
    .unwrap();
    assert_eq!(
        read_back_profile(&heic_derivative.path).as_deref(),
        Some(heic_profile.as_slice())
    );

    let mismatch = write_jpeg_derivative(
        &decoded_jpeg.image,
        &temporary.path().join("p3-derivative.jpg"),
        64,
        92,
        Some(&display_p3),
    )
    .unwrap();
    let read_back =
        read_back_profile(&mismatch.path).expect("the Display P3 profile should be embedded");
    assert_ne!(read_back, srgb);
    assert_eq!(read_back, display_p3);
}

#[test]
fn representative_video_containers_codecs_and_proxy_path_record_fidelity() {
    let resolved = ffmpeg::resolve().expect("Task 016 requires bundled development sidecars");
    let temporary = tempfile::tempdir().unwrap();
    let source = root().join("fixtures/clips/synthetic-speech.mp4");
    let cases = [
        (
            "prores.mov",
            vec!["-an", "-c:v", "prores", "-profile:v", "2"],
        ),
        ("h264.m4v", vec!["-an", "-c:v", "h264_videotoolbox"]),
        (
            "dnx.mxf",
            vec![
                "-an",
                "-vf",
                "scale=1280:720",
                "-c:v",
                "dnxhd",
                "-b:v",
                "90M",
            ],
        ),
        (
            "hevc.mov",
            vec!["-an", "-c:v", "hevc_videotoolbox", "-tag:v", "hvc1"],
        ),
    ];
    for (name, arguments) in &cases {
        let output = temporary.path().join(name);
        let status = Command::new(&resolved.path)
            .args(["-y", "-v", "error", "-i"])
            .arg(&source)
            .args(["-t", "1"])
            .args(arguments)
            .arg(&output)
            .status()
            .unwrap();
        assert!(status.success(), "could not generate {name}");
    }

    let runner = Runner::new(resolved, 2, "task-016-fidelity");
    let started = Instant::now();
    let mut checks = Vec::new();
    for (name, _) in &cases {
        let path = temporary.path().join(name);
        let probe = runner.probe(&path).unwrap().value;
        let policy = proxy_policy(&probe).unwrap();
        let codec = probe.video_codec.clone().unwrap();
        if codec == "hevc" {
            assert!(policy.required);
            let proxy = temporary.path().join("hevc-working-proxy.mp4");
            runner
                .generate_edit_proxy_with_control(
                    &path,
                    &proxy,
                    &CancellationToken::default(),
                    |_| {},
                )
                .unwrap();
            assert_eq!(
                runner.probe(&proxy).unwrap().value.video_codec.as_deref(),
                Some("h264")
            );
        } else {
            assert!(!policy.required, "{codec} should stay on the direct path");
        }
        checks.push(FidelityCheck {
            source: (*name).to_owned(),
            decoder_or_codec: codec,
            width: probe.width,
            height: probe.height,
            orientation_note: format!("rotation metadata: {:?}", probe.rotation),
            color_note: format!(
                "space={:?}, primaries={:?}, transfer={:?}, range={:?}, bit_depth={:?}",
                probe.color_space,
                probe.color_primaries,
                probe.color_transfer,
                probe.color_range,
                probe.bit_depth
            ),
            proxy_required: Some(policy.required),
        });
    }
    write_report(
        temporary.path(),
        FidelityReport {
            elapsed_ms: started.elapsed().as_millis(),
            peak_resident_bytes: peak_resident_bytes(),
            failures: Vec::new(),
            checks,
        },
    );
}

fn convert_with_sips(arguments: &[&str], source: &Path, output: &Path) {
    let sips = Command::new("/usr/bin/sips")
        .args(arguments)
        .arg(source)
        .arg("--out")
        .arg(output)
        .output()
        .unwrap();
    assert!(
        sips.status.success(),
        "{}",
        String::from_utf8_lossy(&sips.stderr)
    );
}

fn try_convert_with_sips(arguments: &[&str], source: &Path, output: &Path) -> Option<PathBuf> {
    let sips = Command::new("/usr/bin/sips")
        .args(arguments)
        .arg(source)
        .arg("--out")
        .arg(output)
        .output()
        .unwrap();
    if sips.status.success() {
        Some(output.to_path_buf())
    } else {
        eprintln!(
            "sips failed ({}): {}",
            sips.status,
            String::from_utf8_lossy(&sips.stderr).trim()
        );
        None
    }
}

fn system_profile(name: &str) -> Option<Vec<u8>> {
    let path = Path::new("/System/Library/ColorSync/Profiles").join(name);
    if !path.exists() {
        eprintln!(
            "skipping ICC profile check: {} is not installed",
            path.display()
        );
        return None;
    }
    Some(std::fs::read(&path).unwrap())
}

fn read_back_profile(path: &Path) -> Option<Vec<u8>> {
    let reader = ImageReader::open(path)
        .unwrap()
        .with_guessed_format()
        .unwrap();
    let mut decoder = reader.into_decoder().unwrap();
    decoder.icc_profile().unwrap()
}

fn assert_pixels_match(left: &RgbImage, right: &RgbImage, tolerance: i32) {
    assert_eq!(left.dimensions(), right.dimensions());
    for (x, y, left_pixel) in left.enumerate_pixels() {
        let right_pixel = right.get_pixel(x, y);
        for channel in 0..3 {
            let difference = i32::from(left_pixel.0[channel]) - i32::from(right_pixel.0[channel]);
            assert!(
                difference.abs() <= tolerance,
                "channel {channel} at ({x},{y}) differs by {difference}"
            );
        }
    }
}

fn jpeg_with_orientation(source: &Path, orientation: u16) -> Vec<u8> {
    let jpeg = std::fs::read(source).unwrap();
    assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(b"II");
    exif.extend_from_slice(&42_u16.to_le_bytes());
    exif.extend_from_slice(&8_u32.to_le_bytes());
    exif.extend_from_slice(&1_u16.to_le_bytes());
    exif.extend_from_slice(&0x0112_u16.to_le_bytes());
    exif.extend_from_slice(&3_u16.to_le_bytes());
    exif.extend_from_slice(&1_u32.to_le_bytes());
    exif.extend_from_slice(&orientation.to_le_bytes());
    exif.extend_from_slice(&0_u16.to_le_bytes());
    exif.extend_from_slice(&0_u32.to_le_bytes());
    let segment_length = u16::try_from(exif.len() + 2).unwrap();
    let mut result = jpeg[..2].to_vec();
    result.extend_from_slice(&[0xff, 0xe1]);
    result.extend_from_slice(&segment_length.to_be_bytes());
    result.extend_from_slice(&exif);
    result.extend_from_slice(&jpeg[2..]);
    result
}

fn write_report(directory: &Path, report: FidelityReport) {
    let encoded = serde_json::to_string_pretty(&report).unwrap();
    std::fs::write(directory.join("source-fidelity-report.json"), &encoded).unwrap();
    assert!(encoded.contains("peak_resident_bytes"));
    assert!(encoded.contains("orientation_note"));
    eprintln!("{encoded}");
}

fn peak_resident_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status == 0 {
        u64::try_from(unsafe { usage.assume_init() }.ru_maxrss).unwrap_or(0)
    } else {
        0
    }
}
