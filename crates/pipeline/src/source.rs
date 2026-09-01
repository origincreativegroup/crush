//! Capability-aware, non-destructive still-image decoding and derivative generation.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
#[cfg(target_os = "macos")]
use std::io::BufReader;
use std::io::Write;
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
use image::codecs::png::{CompressionType as PngCompression, FilterType as PngFilter, PngEncoder};
use image::codecs::tiff::TiffEncoder;
use image::metadata::{Cicp, Orientation};
use image::{
    imageops::FilterType as ResizeFilter, ConvertColorOptions, DynamicImage, ExtendedColorType,
    GenericImageView, ImageDecoder, ImageEncoder, ImageReader, RgbImage, RgbaImage,
};
use moxcms::{ColorProfile, Layout, TransformOptions};
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

/// Crop coordinates in the post-EXIF, post-recipe-rotation image. Values are normalized to the
/// half-open source bounds. Pixel conversion uses floor for the origin and ceil for the far edge
/// so a valid non-empty normalized crop cannot collapse during integer conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedCrop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BasicPhotoGrade {
    /// Stops of exposure adjustment, in the inclusive range -5..=5.
    pub exposure_ev: f64,
    /// Linear-light contrast around middle gray, in the inclusive range -1..=1.
    pub contrast: f64,
    /// Linear-light saturation multiplier, in the inclusive range 0..=2.
    pub saturation: f64,
    /// Blue-to-amber adjustment, in the inclusive range -1..=1.
    pub temperature: f64,
    /// Green-to-magenta adjustment, in the inclusive range -1..=1.
    pub tint: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhotoGrade {
    None,
    Basic(BasicPhotoGrade),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoOutputPreset {
    JpegSrgbV1,
    PngSrgbV1,
    TiffSrgbV1,
}

impl PhotoOutputPreset {
    /// Every preset this renderer supports, in display order. The UI preset catalog is built
    /// from this list — preset facts have exactly one definition.
    pub const ALL: [Self; 3] = [Self::JpegSrgbV1, Self::PngSrgbV1, Self::TiffSrgbV1];

    /// Frozen contract value used in recipes and manifests; never renamed.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::JpegSrgbV1 => "jpeg-srgb-v1",
            Self::PngSrgbV1 => "png-srgb-v1",
            Self::TiffSrgbV1 => "tiff-srgb-v1",
        }
    }

    /// Canonical output extension (save dialogs, destination validation).
    pub const fn extension(self) -> &'static str {
        match self {
            Self::JpegSrgbV1 => "jpg",
            Self::PngSrgbV1 => "png",
            Self::TiffSrgbV1 => "tif",
        }
    }

    /// Every destination extension the preset verifies (JPEG also renders `.jpeg`, TIFF
    /// also renders `.tiff`).
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::JpegSrgbV1 => &["jpg", "jpeg"],
            Self::PngSrgbV1 => &["png"],
            Self::TiffSrgbV1 => &["tif", "tiff"],
        }
    }

    pub const fn media_type(self) -> &'static str {
        match self {
            Self::JpegSrgbV1 => "image/jpeg",
            Self::PngSrgbV1 => "image/png",
            Self::TiffSrgbV1 => "image/tiff",
        }
    }

    /// Human label shown in the UI; also the saved recipe name (frozen contract).
    pub const fn label(self) -> &'static str {
        match self {
            Self::JpegSrgbV1 => "JPEG — smaller, easy to share",
            Self::PngSrgbV1 => "PNG — lossless",
            Self::TiffSrgbV1 => "TIFF — lossless 8-bit copy",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotoRenderRecipe {
    pub crop: Option<NormalizedCrop>,
    pub rotation_degrees: u16,
    pub grade: PhotoGrade,
    pub output: PhotoOutputPreset,
}

impl PhotoRenderRecipe {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            matches!(self.rotation_degrees, 0 | 90 | 180 | 270),
            "photo rotation_degrees must be 0, 90, 180, or 270"
        );
        if let Some(crop) = self.crop {
            let values = [crop.x, crop.y, crop.width, crop.height];
            ensure!(
                values.iter().all(|value| value.is_finite()),
                "photo crop values must be finite"
            );
            ensure!(
                crop.x >= 0.0 && crop.y >= 0.0,
                "photo crop origin must be non-negative"
            );
            ensure!(
                crop.width > 0.0 && crop.height > 0.0,
                "photo crop dimensions must be positive"
            );
            ensure!(
                crop.x + crop.width <= 1.0 && crop.y + crop.height <= 1.0,
                "photo crop must stay inside normalized source bounds"
            );
        }
        if let PhotoGrade::Basic(grade) = self.grade {
            ensure!(
                [
                    grade.exposure_ev,
                    grade.contrast,
                    grade.saturation,
                    grade.temperature,
                    grade.tint,
                ]
                .iter()
                .all(|value| value.is_finite()),
                "photo grade values must be finite"
            );
            ensure!(
                (-5.0..=5.0).contains(&grade.exposure_ev),
                "photo grade exposure_ev must be between -5 and 5"
            );
            ensure!(
                (-1.0..=1.0).contains(&grade.contrast),
                "photo grade contrast must be between -1 and 1"
            );
            ensure!(
                (0.0..=2.0).contains(&grade.saturation),
                "photo grade saturation must be between 0 and 2"
            );
            ensure!(
                (-1.0..=1.0).contains(&grade.temperature),
                "photo grade temperature must be between -1 and 1"
            );
            ensure!(
                (-1.0..=1.0).contains(&grade.tint),
                "photo grade tint must be between -1 and 1"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoRenderResult {
    pub derivative: Derivative,
    pub preset: &'static str,
    pub output_color_space: &'static str,
    pub output_bit_depth: u8,
    pub metadata_policy: &'static str,
    /// Manifest-ready evidence such as `embedded-icc-to-srgb:<sha256>` or
    /// `untagged-assumed-srgb`.
    pub source_color_handling: String,
}

const PHOTO_PRESET_MAX_DIMENSION: u32 = 4096;
const PHOTO_JPEG_QUALITY: u8 = 92;
const PHOTO_OUTPUT_COLOR_SPACE: &str = "sRGB IEC61966-2.1";
const PHOTO_METADATA_POLICY: &str = "strip-exif-iptc-xmp-gps;embed-output-icc";

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

/// Apply the version-one photo recipe to a fully decoded original and write a deterministic,
/// metadata-stripped sRGB derivative. The caller supplies a private/staging output path; this
/// primitive creates it exclusively and never replaces an existing path.
pub fn render_photo_derivative(
    decoded: &DecodedPhoto,
    recipe: &PhotoRenderRecipe,
    output: &Path,
) -> anyhow::Result<PhotoRenderResult> {
    recipe.validate()?;

    let decoded_depth = decoded
        .bit_depth
        .unwrap_or_else(|| image_channel_depth(&decoded.image));
    let buffer_depth = image_channel_depth(&decoded.image);
    ensure!(
        decoded_depth > 0 && decoded_depth <= 8 && buffer_depth > 0 && buffer_depth <= 8,
        "{} cannot render source depth {} (decoded buffer depth {}) to the 8-bit {} preset without silent precision loss",
        recipe.output.as_str(),
        decoded_depth,
        buffer_depth,
        recipe.output.as_str()
    );

    let mut image = match recipe.rotation_degrees {
        0 => decoded.image.clone(),
        90 => decoded.image.rotate90(),
        180 => decoded.image.rotate180(),
        270 => decoded.image.rotate270(),
        _ => unreachable!("recipe validation accepts only right-angle rotations"),
    };
    if let Some(crop) = recipe.crop {
        image = apply_normalized_crop(&image, crop)?;
    }
    let (mut image, source_color_handling) = convert_photo_to_srgb(image, decoded)?;
    image = apply_photo_grade(image, recipe.grade);
    if image.width().max(image.height()) > PHOTO_PRESET_MAX_DIMENSION {
        image = image.resize(
            PHOTO_PRESET_MAX_DIMENSION,
            PHOTO_PRESET_MAX_DIMENSION,
            ResizeFilter::Lanczos3,
        );
    }
    if recipe.output == PhotoOutputPreset::JpegSrgbV1 {
        ensure!(
            !has_transparency(&image),
            "jpeg-srgb-v1 cannot preserve transparency and no flattening background is declared"
        );
    }

    let srgb_profile = deterministic_srgb_profile_bytes()?;
    encode_photo_preset(&image, recipe.output, output, &srgb_profile)?;
    let (width, height) = image.dimensions();
    Ok(PhotoRenderResult {
        derivative: Derivative {
            path: output.to_path_buf(),
            width,
            height,
            sha256: crate::sha256_file(output)?,
        },
        preset: recipe.output.as_str(),
        output_color_space: PHOTO_OUTPUT_COLOR_SPACE,
        output_bit_depth: 8,
        metadata_policy: PHOTO_METADATA_POLICY,
        source_color_handling,
    })
}

fn deterministic_srgb_profile_bytes() -> anyhow::Result<Vec<u8>> {
    let mut profile = ColorProfile::new_srgb()
        .encode()
        .context("failed to create the sRGB output profile")?;
    // moxcms 0.8.1's writer currently writes `ColorDateTime::now()` into ICC header bytes
    // 24..36 even when the public profile field is set. Creation time is metadata, not
    // colorimetry, so canonicalize the encoded header itself to 2000-01-01T00:00:00. A pinned
    // dependency plus this byte-level normalization makes output stable across processes.
    ensure!(
        profile.len() >= 36,
        "encoded sRGB output profile is shorter than its ICC header"
    );
    profile[24..36].copy_from_slice(&[
        0x07, 0xd0, // year 2000
        0x00, 0x01, // month 1
        0x00, 0x01, // day 1
        0x00, 0x00, // hour 0
        0x00, 0x00, // minute 0
        0x00, 0x00, // second 0
    ]);
    Ok(profile)
}

fn image_channel_depth(image: &DynamicImage) -> i64 {
    let color = image.color();
    i64::from(color.bits_per_pixel() / u16::from(color.channel_count()))
}

fn apply_normalized_crop(
    image: &DynamicImage,
    crop: NormalizedCrop,
) -> anyhow::Result<DynamicImage> {
    let width = image.width();
    let height = image.height();
    ensure!(width > 0 && height > 0, "cannot crop an empty photo");
    let left = (crop.x * f64::from(width)).floor() as u32;
    let top = (crop.y * f64::from(height)).floor() as u32;
    let right = ((crop.x + crop.width) * f64::from(width)).ceil() as u32;
    let bottom = ((crop.y + crop.height) * f64::from(height)).ceil() as u32;
    let right = right.min(width);
    let bottom = bottom.min(height);
    ensure!(
        right > left && bottom > top,
        "normalized photo crop collapsed to an empty pixel region"
    );
    Ok(image.crop_imm(left, top, right - left, bottom - top))
}

fn convert_photo_to_srgb(
    image: DynamicImage,
    decoded: &DecodedPhoto,
) -> anyhow::Result<(DynamicImage, String)> {
    if let Some(profile_bytes) = decoded.icc_profile.as_deref() {
        let source_profile = ColorProfile::new_from_slice(profile_bytes)
            .context("source ICC profile is unsupported or malformed")?;
        let target_profile = ColorProfile::new_srgb();
        let (layout, source, width, height, has_alpha) = if image.has_alpha() {
            let pixels = image.to_rgba8();
            (
                Layout::Rgba,
                pixels.into_raw(),
                image.width(),
                image.height(),
                true,
            )
        } else {
            let pixels = image.to_rgb8();
            (
                Layout::Rgb,
                pixels.into_raw(),
                image.width(),
                image.height(),
                false,
            )
        };
        let transform = source_profile
            .create_transform_8bit(layout, &target_profile, layout, TransformOptions::default())
            .context("source ICC profile cannot be converted to the sRGB output profile")?;
        let mut converted = vec![0_u8; source.len()];
        transform
            .transform(&source, &mut converted)
            .context("source ICC to sRGB pixel conversion failed")?;
        let image = if has_alpha {
            DynamicImage::ImageRgba8(
                RgbaImage::from_raw(width, height, converted)
                    .context("ICC conversion returned an invalid RGBA buffer")?,
            )
        } else {
            DynamicImage::ImageRgb8(
                RgbImage::from_raw(width, height, converted)
                    .context("ICC conversion returned an invalid RGB buffer")?,
            )
        };
        let profile_hash = sha256_bytes(profile_bytes);
        if let Some(recorded_hash) = decoded.icc_profile_sha256.as_deref() {
            ensure!(
                recorded_hash == profile_hash,
                "source ICC profile bytes do not match the recorded SHA-256"
            );
        }
        return Ok((image, format!("embedded-icc-to-srgb:{profile_hash}")));
    }

    if image.color_space() != Cicp::SRGB {
        let source_cicp = format!("{:?}", image.color_space());
        let mut converted = image;
        converted
            .apply_color_space(Cicp::SRGB, ConvertColorOptions::default())
            .with_context(|| {
                format!("source CICP color space {source_cicp} cannot be converted to sRGB")
            })?;
        return Ok((
            canonical_rgb8(converted),
            format!("cicp-to-srgb:{source_cicp}"),
        ));
    }

    let declared_color = decoded
        .icc_profile_name
        .as_deref()
        .or(decoded.color_space.as_deref());
    if let Some(declared_color) = declared_color {
        ensure!(
            is_srgb_label(declared_color),
            "source declares color space/profile {declared_color:?}, but supplies no ICC profile that can be converted to sRGB"
        );
        return Ok((
            canonical_rgb8(image),
            format!("declared-srgb:{declared_color}"),
        ));
    }

    Ok((canonical_rgb8(image), "untagged-assumed-srgb".to_owned()))
}

fn is_srgb_label(value: &str) -> bool {
    let normalized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("srgb") || normalized.contains("iec6196621")
}

fn canonical_rgb8(image: DynamicImage) -> DynamicImage {
    if image.has_alpha() {
        DynamicImage::ImageRgba8(image.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(image.to_rgb8())
    }
}

fn has_transparency(image: &DynamicImage) -> bool {
    image.has_alpha() && image.to_rgba8().pixels().any(|pixel| pixel[3] != 255)
}

fn apply_photo_grade(image: DynamicImage, grade: PhotoGrade) -> DynamicImage {
    let PhotoGrade::Basic(grade) = grade else {
        return image;
    };
    if image.has_alpha() {
        let mut pixels = image.to_rgba8();
        for pixel in pixels.pixels_mut() {
            let graded = grade_srgb_pixel([pixel[0], pixel[1], pixel[2]], grade);
            pixel[0] = graded[0];
            pixel[1] = graded[1];
            pixel[2] = graded[2];
        }
        DynamicImage::ImageRgba8(pixels)
    } else {
        let mut pixels = image.to_rgb8();
        for pixel in pixels.pixels_mut() {
            pixel.0 = grade_srgb_pixel(pixel.0, grade);
        }
        DynamicImage::ImageRgb8(pixels)
    }
}

fn grade_srgb_pixel(pixel: [u8; 3], grade: BasicPhotoGrade) -> [u8; 3] {
    let mut linear = pixel.map(srgb_byte_to_linear);
    // White-balance controls are deliberately bounded, channel-explicit multipliers. Positive
    // temperature warms (red up, blue down); positive tint moves green toward magenta.
    linear[0] *= 1.0 + 0.12 * grade.temperature + 0.06 * grade.tint;
    linear[1] *= 1.0 - 0.12 * grade.tint;
    linear[2] *= 1.0 - 0.12 * grade.temperature + 0.06 * grade.tint;
    let exposure = 2.0_f64.powf(grade.exposure_ev);
    for channel in &mut linear {
        *channel *= exposure;
        *channel = (*channel - 0.18) * (1.0 + grade.contrast) + 0.18;
    }
    let luminance = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
    for channel in &mut linear {
        *channel = luminance + (*channel - luminance) * grade.saturation;
    }
    linear.map(linear_to_srgb_byte)
}

fn srgb_byte_to_linear(value: u8) -> f64 {
    let encoded = f64::from(value) / 255.0;
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_byte(value: f64) -> u8 {
    let linear = value.clamp(0.0, 1.0);
    let encoded = if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

fn encode_photo_preset(
    image: &DynamicImage,
    preset: PhotoOutputPreset,
    output: &Path,
    srgb_profile: &[u8],
) -> anyhow::Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create photo render directory {}",
                parent.display()
            )
        })?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .with_context(|| {
            format!(
                "photo render destination already exists or cannot be created: {}",
                output.display()
            )
        })?;

    let encoded = (|| -> anyhow::Result<()> {
        match preset {
            PhotoOutputPreset::JpegSrgbV1 => {
                let pixels = image.to_rgb8();
                let mut encoder = JpegEncoder::new_with_quality(&mut file, PHOTO_JPEG_QUALITY);
                encoder
                    .set_icc_profile(srgb_profile.to_vec())
                    .context("JPEG encoder rejected the sRGB output profile")?;
                encoder
                    .write_image(
                        pixels.as_raw(),
                        pixels.width(),
                        pixels.height(),
                        ExtendedColorType::Rgb8,
                    )
                    .context("failed to encode jpeg-srgb-v1")?;
            }
            PhotoOutputPreset::PngSrgbV1 => {
                let mut encoder = PngEncoder::new_with_quality(
                    &mut file,
                    PngCompression::Best,
                    PngFilter::Adaptive,
                );
                encoder
                    .set_icc_profile(srgb_profile.to_vec())
                    .context("PNG encoder rejected the sRGB output profile")?;
                if image.has_alpha() {
                    let pixels = image.to_rgba8();
                    encoder.write_image(
                        pixels.as_raw(),
                        pixels.width(),
                        pixels.height(),
                        ExtendedColorType::Rgba8,
                    )?;
                } else {
                    let pixels = image.to_rgb8();
                    encoder.write_image(
                        pixels.as_raw(),
                        pixels.width(),
                        pixels.height(),
                        ExtendedColorType::Rgb8,
                    )?;
                }
            }
            PhotoOutputPreset::TiffSrgbV1 => {
                let mut encoder = TiffEncoder::new(&mut file);
                encoder
                    .set_icc_profile(srgb_profile.to_vec())
                    .context("TIFF encoder rejected the sRGB output profile")?;
                if image.has_alpha() {
                    let pixels = image.to_rgba8();
                    encoder.write_image(
                        pixels.as_raw(),
                        pixels.width(),
                        pixels.height(),
                        ExtendedColorType::Rgba8,
                    )?;
                } else {
                    let pixels = image.to_rgb8();
                    encoder.write_image(
                        pixels.as_raw(),
                        pixels.width(),
                        pixels.height(),
                        ExtendedColorType::Rgb8,
                    )?;
                }
            }
        }
        file.flush().context("failed to flush photo derivative")?;
        file.sync_all().context("failed to sync photo derivative")?;
        Ok(())
    })();
    if let Err(error) = encoded {
        drop(file);
        let _ = std::fs::remove_file(output);
        return Err(error).with_context(|| {
            format!(
                "failed to write {} derivative {}",
                preset.as_str(),
                output.display()
            )
        });
    }
    Ok(())
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
    fn photo_recipe_rotates_then_crops_in_normalized_oriented_space() {
        let temporary = tempfile::tempdir().unwrap();
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(4, 2, |x, y| {
            Rgb([(x * 50) as u8, (y * 100) as u8, (x + y) as u8])
        }));
        let decoded = test_decoded_photo(image.clone(), Some(8), None, None);
        let recipe = PhotoRenderRecipe {
            crop: Some(NormalizedCrop {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 0.5,
            }),
            rotation_degrees: 90,
            grade: PhotoGrade::None,
            output: PhotoOutputPreset::PngSrgbV1,
        };
        let output = temporary.path().join("rotated-crop.png");
        let result = render_photo_derivative(&decoded, &recipe, &output).unwrap();
        assert_eq!((result.derivative.width, result.derivative.height), (2, 2));
        let actual = ImageReader::open(output)
            .unwrap()
            .decode()
            .unwrap()
            .to_rgb8();
        let expected = image.rotate90().crop_imm(0, 0, 2, 2).to_rgb8();
        assert_eq!(actual, expected);
    }

    #[test]
    fn photo_presets_are_byte_deterministic_strip_private_metadata_and_embed_srgb() {
        let temporary = tempfile::tempdir().unwrap();
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(24, 16, |x, y| {
            Rgb([(x * 9) as u8, (y * 13) as u8, ((x + y) * 5) as u8])
        }));
        let decoded = test_decoded_photo(image, Some(8), None, None);
        let expected_profile = deterministic_srgb_profile_bytes().unwrap();
        for (preset, extension) in [
            (PhotoOutputPreset::JpegSrgbV1, "jpg"),
            (PhotoOutputPreset::PngSrgbV1, "png"),
            (PhotoOutputPreset::TiffSrgbV1, "tiff"),
        ] {
            let recipe = PhotoRenderRecipe {
                crop: None,
                rotation_degrees: 0,
                grade: PhotoGrade::Basic(BasicPhotoGrade {
                    exposure_ev: 0.25,
                    contrast: 0.1,
                    saturation: 1.15,
                    temperature: 0.2,
                    tint: -0.1,
                }),
                output: preset,
            };
            let first_path = temporary.path().join(format!("first.{extension}"));
            let second_path = temporary.path().join(format!("second.{extension}"));
            let first = render_photo_derivative(&decoded, &recipe, &first_path).unwrap();
            let second = render_photo_derivative(&decoded, &recipe, &second_path).unwrap();
            assert_eq!(first.derivative.sha256, second.derivative.sha256);
            assert_eq!(first.output_color_space, PHOTO_OUTPUT_COLOR_SPACE);
            assert_eq!(first.output_bit_depth, 8);
            assert_eq!(first.metadata_policy, PHOTO_METADATA_POLICY);
            assert_eq!(first.source_color_handling, "untagged-assumed-srgb");

            let reader = ImageReader::open(&first_path)
                .unwrap()
                .with_guessed_format()
                .unwrap();
            let mut decoder = reader.into_decoder().unwrap();
            if preset == PhotoOutputPreset::TiffSrgbV1 {
                // image-rs writes TIFF tag 34675 but its TIFF decoder does not expose ICC data.
                let bytes = std::fs::read(&first_path).unwrap();
                assert!(
                    bytes
                        .windows(expected_profile.len())
                        .any(|window| window == expected_profile),
                    "{} did not retain the declared output profile",
                    preset.as_str()
                );
            } else {
                assert_eq!(
                    decoder.icc_profile().unwrap().as_deref(),
                    Some(expected_profile.as_slice()),
                    "{} did not retain the declared output profile",
                    preset.as_str()
                );
            }
            assert!(decoder.exif_metadata().unwrap().is_none());
        }
    }

    #[test]
    fn embedded_profile_is_converted_to_srgb_instead_of_relabelled() {
        let temporary = tempfile::tempdir().unwrap();
        let display_p3 = ColorProfile::new_display_p3().encode().unwrap();
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(2, 2, Rgb([190, 80, 40])));
        let decoded = test_decoded_photo(
            image,
            Some(8),
            Some("Display P3".to_owned()),
            Some(display_p3),
        );
        let output = temporary.path().join("converted.png");
        let result = render_photo_derivative(
            &decoded,
            &PhotoRenderRecipe {
                crop: None,
                rotation_degrees: 0,
                grade: PhotoGrade::None,
                output: PhotoOutputPreset::PngSrgbV1,
            },
            &output,
        )
        .unwrap();
        assert!(result
            .source_color_handling
            .starts_with("embedded-icc-to-srgb:"));
        let reader = ImageReader::open(output)
            .unwrap()
            .with_guessed_format()
            .unwrap();
        let mut decoder = reader.into_decoder().unwrap();
        assert_eq!(
            decoder.icc_profile().unwrap().as_deref(),
            Some(deterministic_srgb_profile_bytes().unwrap().as_slice())
        );
    }

    #[test]
    fn declared_cicp_is_converted_to_srgb_without_an_icc_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let mut image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(2, 2, Rgb([80, 90, 100])));
        image.set_color_space(Cicp::DISPLAY_P3).unwrap();
        let decoded = test_decoded_photo(image, Some(8), Some("Display P3".to_owned()), None);
        let output = temporary.path().join("cicp-converted.png");
        let result = render_photo_derivative(
            &decoded,
            &PhotoRenderRecipe {
                crop: None,
                rotation_degrees: 0,
                grade: PhotoGrade::None,
                output: PhotoOutputPreset::PngSrgbV1,
            },
            &output,
        )
        .unwrap();
        assert!(result.source_color_handling.starts_with("cicp-to-srgb:"));
    }

    #[test]
    fn unsupported_depth_and_unconvertible_color_fail_without_output() {
        let temporary = tempfile::tempdir().unwrap();
        let recipe = PhotoRenderRecipe {
            crop: None,
            rotation_degrees: 0,
            grade: PhotoGrade::None,
            output: PhotoOutputPreset::TiffSrgbV1,
        };
        let high_depth = test_decoded_photo(
            DynamicImage::ImageRgb8(ImageBuffer::from_pixel(2, 2, Rgb([1, 2, 3]))),
            Some(16),
            Some("sRGB".to_owned()),
            None,
        );
        let high_depth_output = temporary.path().join("high-depth.tiff");
        let error = render_photo_derivative(&high_depth, &recipe, &high_depth_output)
            .unwrap_err()
            .to_string();
        assert!(error.contains("without silent precision loss"), "{error}");
        assert!(!high_depth_output.exists());

        let unsupported_color = test_decoded_photo(
            DynamicImage::ImageRgb8(ImageBuffer::from_pixel(2, 2, Rgb([1, 2, 3]))),
            Some(8),
            Some("Display P3".to_owned()),
            None,
        );
        let unsupported_color_output = temporary.path().join("unsupported-color.tiff");
        let error = render_photo_derivative(&unsupported_color, &recipe, &unsupported_color_output)
            .unwrap_err()
            .to_string();
        assert!(error.contains("supplies no ICC profile"), "{error}");
        assert!(!unsupported_color_output.exists());

        let transparent = test_decoded_photo(
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, image::Rgba([1, 2, 3, 128]))),
            Some(8),
            None,
            None,
        );
        let transparent_output = temporary.path().join("transparent.jpg");
        let error = render_photo_derivative(
            &transparent,
            &PhotoRenderRecipe {
                output: PhotoOutputPreset::JpegSrgbV1,
                ..recipe
            },
            &transparent_output,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("cannot preserve transparency"), "{error}");
        assert!(!transparent_output.exists());
    }

    #[test]
    fn invalid_recipe_and_existing_destination_fail_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let decoded = test_decoded_photo(
            DynamicImage::ImageRgb8(ImageBuffer::from_pixel(2, 2, Rgb([1, 2, 3]))),
            Some(8),
            None,
            None,
        );
        let invalid = PhotoRenderRecipe {
            crop: Some(NormalizedCrop {
                x: 0.8,
                y: 0.0,
                width: 0.3,
                height: 1.0,
            }),
            rotation_degrees: 45,
            grade: PhotoGrade::Basic(BasicPhotoGrade {
                exposure_ev: 6.0,
                contrast: 0.0,
                saturation: 1.0,
                temperature: 0.0,
                tint: 0.0,
            }),
            output: PhotoOutputPreset::PngSrgbV1,
        };
        assert!(invalid.validate().is_err());

        let output = temporary.path().join("existing.png");
        std::fs::write(&output, b"user data").unwrap();
        let original_hash = crate::sha256_file(&output).unwrap();
        let error = render_photo_derivative(
            &decoded,
            &PhotoRenderRecipe {
                crop: None,
                rotation_degrees: 0,
                grade: PhotoGrade::None,
                output: PhotoOutputPreset::PngSrgbV1,
            },
            &output,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(crate::sha256_file(&output).unwrap(), original_hash);
    }

    fn test_decoded_photo(
        image: DynamicImage,
        bit_depth: Option<i64>,
        color_space: Option<String>,
        icc_profile: Option<Vec<u8>>,
    ) -> DecodedPhoto {
        DecodedPhoto {
            image,
            source_format: "test".to_owned(),
            decoder: "test".to_owned(),
            proxy_provenance: PhotoProxyProvenance::DecodedOriginal,
            orientation: None,
            orientation_applied: true,
            captured_at: None,
            camera_make: None,
            camera_model: None,
            lens: None,
            bit_depth,
            color_space,
            icc_profile_name: None,
            icc_profile_sha256: icc_profile.as_deref().map(sha256_bytes),
            icc_profile,
            exposure_json: "{}".to_owned(),
            gps_present: false,
            metadata_json: "{}".to_owned(),
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
