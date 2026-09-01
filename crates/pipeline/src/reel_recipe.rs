//! Backend-neutral normalization for frozen Reel Studio reel recipes.
//!
//! Reel Studio timing is relative to each manually reviewed library segment. Resolution maps
//! those exact segment spans back to originals; it never substitutes Crush scene boundaries.

use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{bail, ensure, Context};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelProvenanceOrigin {
    General,
    Personal,
    Historical,
    Imported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReelProvenance {
    pub origin: ReelProvenanceOrigin,
    pub source: String,
    pub external_id: Option<String>,
    pub profile_version: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelVibe {
    Bright,
    Electro,
    Trap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelFormat {
    Portrait9x16,
    Portrait4x5,
    Square,
    Landscape16x9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatermarkPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionPosition {
    Low,
    Mid,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelTransition {
    Cut,
    Mix,
    Fade,
    White,
    SlideLeft,
    SlideRight,
    SlideUp,
    WipeLeft,
    Circle,
    BlurMix,
    Whip,
    Zoom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelMotion {
    None,
    In,
    Out,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReelOutputPreset {
    Mp4H264SdrV1,
    MovH264SdrV1,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropKeyframe {
    /// Seconds within the Reel Studio library segment, not the original source timeline.
    pub segment_time_s: f64,
    pub x: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReelGrade {
    pub exposure: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub warmth: f64,
    pub hue: f64,
    pub vibrance: f64,
    pub shadows: f64,
    pub highlights: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReelSequenceItem {
    /// Reel Studio `segments.segment_id`.
    pub id: String,
    /// Seconds within the manually reviewed segment.
    pub segment_in_s: f64,
    pub segment_out_s: f64,
    pub crop_x: f64,
    pub crop_keyframes: Vec<CropKeyframe>,
    pub caption: Option<String>,
    pub caption_position: CaptionPosition,
    pub transition: ReelTransition,
    pub speed: f64,
    pub motion: ReelMotion,
    pub clip_volume: f64,
    pub grade: ReelGrade,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReelCover {
    pub segment_id: String,
    /// Seconds within the Reel Studio library segment.
    pub segment_time_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrozenReelRecipeV2 {
    pub provenance: ReelProvenance,
    pub theme: Option<String>,
    pub vibe: Option<ReelVibe>,
    pub music: Option<String>,
    pub target_seconds: Option<f64>,
    pub beat_snap: bool,
    pub format: ReelFormat,
    pub music_volume: f64,
    pub watermark: Option<WatermarkPosition>,
    pub cover: Option<ReelCover>,
    pub sequence: Vec<ReelSequenceItem>,
    /// Confirmed Reel Studio crop write-backs, keyed by segment id.
    pub crops: BTreeMap<String, f64>,
    pub output: ReelOutputPreset,
}

/// Exact original-source placement for one Reel Studio segment.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentSourceSpan {
    pub original_source_id: String,
    pub original_path: PathBuf,
    /// Segment boundary on the original source timeline.
    pub base_start_s: f64,
    pub base_end_s: f64,
}

impl SegmentSourceSpan {
    pub fn new(
        original_source_id: impl Into<String>,
        original_path: impl Into<PathBuf>,
        base_start_s: f64,
        base_end_s: f64,
    ) -> anyhow::Result<Self> {
        ensure!(
            base_start_s.is_finite()
                && base_end_s.is_finite()
                && base_start_s >= 0.0
                && base_end_s > base_start_s,
            "segment source end must exceed its finite non-negative start"
        );
        let original_path = original_path.into();
        ensure!(
            !original_path.as_os_str().is_empty(),
            "original source path must not be empty"
        );
        Ok(Self {
            original_source_id: nonempty_owned(original_source_id.into(), "original source id")?,
            original_path,
            base_start_s,
            base_end_s,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedReelItem {
    pub item: ReelSequenceItem,
    pub original_source_id: String,
    pub original_path: PathBuf,
    pub segment_base_start_s: f64,
    pub segment_base_end_s: f64,
    /// Absolute boundaries on the original source timeline.
    pub source_in_s: f64,
    pub source_out_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedReelCover {
    pub cover: ReelCover,
    pub original_source_id: String,
    pub original_path: PathBuf,
    pub source_time_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedReelRecipe {
    pub recipe: FrozenReelRecipeV2,
    pub sequence: Vec<ResolvedReelItem>,
    pub cover: Option<ResolvedReelCover>,
}

pub fn parse_frozen_reel_recipe_v2(value: &str) -> anyhow::Result<FrozenReelRecipeV2> {
    let parsed: Value =
        serde_json::from_str(value).context("frozen reel recipe is invalid JSON")?;
    let object = parsed
        .as_object()
        .context("frozen reel recipe must be an object")?;
    exact_keys(
        object,
        &[
            "schema_version",
            "kind",
            "provenance",
            "theme",
            "vibe",
            "music",
            "target_seconds",
            "beat_snap",
            "format",
            "music_volume",
            "watermark",
            "cover",
            "sequence",
            "crops",
            "output",
        ],
        "frozen reel recipe",
    )?;
    ensure!(
        object.get("schema_version").and_then(Value::as_u64) == Some(2),
        "frozen reel recipe schema_version must be integer 2"
    );
    ensure!(
        string(object, "kind", "frozen reel recipe")? == "reel",
        "frozen reel recipe kind must be reel"
    );

    let sequence_values = object
        .get("sequence")
        .and_then(Value::as_array)
        .context("reel sequence must be an array")?;
    ensure!(
        !sequence_values.is_empty(),
        "reel sequence must not be empty"
    );
    let mut sequence = Vec::with_capacity(sequence_values.len());
    let mut item_crops = BTreeMap::new();
    for (index, value) in sequence_values.iter().enumerate() {
        let item = parse_sequence_item(value)
            .with_context(|| format!("invalid reel sequence item {index}"))?;
        ensure!(
            item_crops.insert(item.id.clone(), item.crop_x).is_none(),
            "reel sequence contains duplicate id {:?}",
            item.id
        );
        sequence.push(item);
    }

    let crops = parse_crops(object.get("crops").expect("key set checked"), &item_crops)?;
    let cover = parse_cover(object.get("cover").expect("key set checked"), &sequence)?;
    Ok(FrozenReelRecipeV2 {
        provenance: parse_provenance(object.get("provenance").expect("key set checked"))?,
        theme: nullable_string(object.get("theme").expect("key set checked"), "reel theme")?,
        vibe: parse_vibe(object.get("vibe").expect("key set checked"))?,
        music: nullable_relative_path(object.get("music").expect("key set checked"), "reel music")?,
        target_seconds: nullable_positive_number(
            object.get("target_seconds").expect("key set checked"),
            "reel target_seconds",
        )?,
        beat_snap: object
            .get("beat_snap")
            .and_then(Value::as_bool)
            .context("reel beat_snap must be a boolean")?,
        format: parse_format(string(object, "format", "reel")?)?,
        music_volume: percentage(
            object.get("music_volume").expect("key set checked"),
            "music volume",
        )?,
        watermark: parse_watermark(object.get("watermark").expect("key set checked"))?,
        cover,
        sequence,
        crops,
        output: parse_output(object.get("output").expect("key set checked"))?,
    })
}

pub fn resolve_reel_recipe(
    recipe: &FrozenReelRecipeV2,
    segments: &BTreeMap<String, SegmentSourceSpan>,
) -> anyhow::Result<ResolvedReelRecipe> {
    let mut sequence = Vec::with_capacity(recipe.sequence.len());
    for item in &recipe.sequence {
        let source = segments.get(&item.id).with_context(|| {
            format!("reel segment {:?} has no original-source mapping", item.id)
        })?;
        let base_start = source.base_start_s;
        let base_end = source.base_end_s;
        let segment_duration = base_end - base_start;
        ensure!(
            item.segment_out_s <= segment_duration,
            "reel segment {:?} out {} exceeds mapped segment duration {}",
            item.id,
            item.segment_out_s,
            segment_duration
        );
        sequence.push(ResolvedReelItem {
            item: item.clone(),
            original_source_id: source.original_source_id.clone(),
            original_path: source.original_path.clone(),
            segment_base_start_s: base_start,
            segment_base_end_s: base_end,
            source_in_s: base_start + item.segment_in_s,
            source_out_s: base_start + item.segment_out_s,
        });
    }
    let cover = recipe
        .cover
        .as_ref()
        .map(|cover| {
            let source = segments.get(&cover.segment_id).with_context(|| {
                format!(
                    "reel cover segment {:?} has no original-source mapping",
                    cover.segment_id
                )
            })?;
            let base_start = source.base_start_s;
            let base_end = source.base_end_s;
            ensure!(
                cover.segment_time_s <= base_end - base_start,
                "reel cover time exceeds mapped segment duration"
            );
            Ok(ResolvedReelCover {
                cover: cover.clone(),
                original_source_id: source.original_source_id.clone(),
                original_path: source.original_path.clone(),
                source_time_s: base_start + cover.segment_time_s,
            })
        })
        .transpose()?;
    Ok(ResolvedReelRecipe {
        recipe: recipe.clone(),
        sequence,
        cover,
    })
}

fn parse_sequence_item(value: &Value) -> anyhow::Result<ReelSequenceItem> {
    let object = value
        .as_object()
        .context("reel sequence item must be an object")?;
    exact_keys(
        object,
        &[
            "id",
            "in",
            "out",
            "crop_x",
            "crop_kf",
            "caption",
            "cap_pos",
            "transition",
            "speed",
            "motion",
            "clip_volume",
            "grade",
        ],
        "reel sequence item",
    )?;
    let segment_in_s = number(object, "in", "reel sequence item")?;
    let segment_out_s = number(object, "out", "reel sequence item")?;
    ensure!(
        segment_in_s >= 0.0 && segment_out_s > segment_in_s,
        "reel item out must exceed non-negative in"
    );
    let crop_x = unit(number(object, "crop_x", "reel sequence item")?, "crop_x")?;
    let keyframes = object
        .get("crop_kf")
        .and_then(Value::as_array)
        .context("reel crop_kf must be an array")?;
    let mut crop_keyframes = Vec::with_capacity(keyframes.len());
    let mut previous = None;
    for keyframe in keyframes {
        let keyframe = keyframe
            .as_object()
            .context("reel crop keyframe must be an object")?;
        exact_keys(keyframe, &["t", "x"], "reel crop keyframe")?;
        let time = number(keyframe, "t", "reel crop keyframe")?;
        ensure!(
            (segment_in_s..=segment_out_s).contains(&time),
            "crop keyframe time must stay inside the item in/out span"
        );
        if let Some(previous) = previous {
            ensure!(time > previous, "crop keyframe times must increase");
        }
        previous = Some(time);
        crop_keyframes.push(CropKeyframe {
            segment_time_s: time,
            x: unit(
                number(keyframe, "x", "reel crop keyframe")?,
                "crop keyframe x",
            )?,
        });
    }
    Ok(ReelSequenceItem {
        id: nonempty_owned(
            string(object, "id", "reel sequence item")?.to_owned(),
            "segment id",
        )?,
        segment_in_s,
        segment_out_s,
        crop_x,
        crop_keyframes,
        caption: nullable_string(object.get("caption").expect("key set checked"), "caption")?,
        caption_position: parse_caption_position(string(object, "cap_pos", "reel sequence item")?)?,
        transition: parse_transition(string(object, "transition", "reel sequence item")?)?,
        speed: bounded(
            number(object, "speed", "reel sequence item")?,
            0.5,
            2.0,
            "speed",
        )?,
        motion: parse_motion(string(object, "motion", "reel sequence item")?)?,
        clip_volume: percentage(
            object.get("clip_volume").expect("key set checked"),
            "clip volume",
        )?,
        grade: parse_grade(object.get("grade").expect("key set checked"))?,
    })
}

fn parse_provenance(value: &Value) -> anyhow::Result<ReelProvenance> {
    let object = value
        .as_object()
        .context("reel provenance must be an object")?;
    exact_keys(
        object,
        &["origin", "source", "external_id", "profile_version"],
        "reel provenance",
    )?;
    let origin = match string(object, "origin", "reel provenance")? {
        "general" => ReelProvenanceOrigin::General,
        "personal" => ReelProvenanceOrigin::Personal,
        "historical" => ReelProvenanceOrigin::Historical,
        "imported" => ReelProvenanceOrigin::Imported,
        other => bail!("unsupported reel provenance origin {other:?}"),
    };
    let external_id = nullable_string(
        object.get("external_id").expect("key set checked"),
        "external id",
    )?;
    let profile_version = if object
        .get("profile_version")
        .expect("key set checked")
        .is_null()
    {
        None
    } else {
        Some(
            object
                .get("profile_version")
                .and_then(Value::as_i64)
                .filter(|version| *version > 0)
                .context("profile_version must be null or a positive integer")?,
        )
    };
    ensure!(
        (origin == ReelProvenanceOrigin::Personal) == profile_version.is_some(),
        "personal provenance requires profile_version and no other origin may carry it"
    );
    ensure!(
        !matches!(
            origin,
            ReelProvenanceOrigin::Historical | ReelProvenanceOrigin::Imported
        ) || external_id.is_some(),
        "historical/imported provenance requires external_id"
    );
    Ok(ReelProvenance {
        origin,
        source: nonempty_owned(
            string(object, "source", "reel provenance")?.to_owned(),
            "provenance source",
        )?,
        external_id,
        profile_version,
    })
}

fn parse_cover(value: &Value, sequence: &[ReelSequenceItem]) -> anyhow::Result<Option<ReelCover>> {
    if value.is_null() {
        return Ok(None);
    }
    let object = value.as_object().context("reel cover must be an object")?;
    exact_keys(object, &["id", "time"], "reel cover")?;
    let id = string(object, "id", "reel cover")?;
    let time = number(object, "time", "reel cover")?;
    let item = sequence
        .iter()
        .find(|item| item.id == id)
        .with_context(|| format!("reel cover references unknown segment {id:?}"))?;
    ensure!(
        (item.segment_in_s..=item.segment_out_s).contains(&time),
        "reel cover time must stay inside the item in/out span"
    );
    Ok(Some(ReelCover {
        segment_id: id.to_owned(),
        segment_time_s: time,
    }))
}

fn parse_crops(
    value: &Value,
    item_crops: &BTreeMap<String, f64>,
) -> anyhow::Result<BTreeMap<String, f64>> {
    let object = value.as_object().context("reel crops must be an object")?;
    let mut crops = BTreeMap::new();
    for (id, value) in object {
        let crop = unit(
            value.as_f64().context("reel crop must be a number")?,
            "reel crop",
        )?;
        ensure!(
            item_crops.get(id).is_some_and(|item| *item == crop),
            "reel crop for {id:?} must match a sequence item crop_x"
        );
        crops.insert(id.clone(), crop);
    }
    Ok(crops)
}

fn parse_grade(value: &Value) -> anyhow::Result<ReelGrade> {
    let object = value.as_object().context("reel grade must be an object")?;
    exact_keys(
        object,
        &["b", "c", "s", "t", "h", "v", "sh", "hl"],
        "reel grade",
    )?;
    Ok(ReelGrade {
        exposure: bounded(number(object, "b", "grade")?, 60.0, 140.0, "grade b")?,
        contrast: bounded(number(object, "c", "grade")?, 60.0, 140.0, "grade c")?,
        saturation: bounded(number(object, "s", "grade")?, 0.0, 180.0, "grade s")?,
        warmth: bounded(number(object, "t", "grade")?, -50.0, 50.0, "grade t")?,
        hue: bounded(number(object, "h", "grade")?, -30.0, 30.0, "grade h")?,
        vibrance: bounded(number(object, "v", "grade")?, -50.0, 50.0, "grade v")?,
        shadows: bounded(number(object, "sh", "grade")?, -50.0, 50.0, "grade sh")?,
        highlights: bounded(number(object, "hl", "grade")?, -50.0, 50.0, "grade hl")?,
    })
}

fn parse_output(value: &Value) -> anyhow::Result<ReelOutputPreset> {
    let object = value.as_object().context("reel output must be an object")?;
    exact_keys(object, &["preset"], "reel output")?;
    match string(object, "preset", "reel output")? {
        "mp4-h264-sdr-v1" => Ok(ReelOutputPreset::Mp4H264SdrV1),
        "mov-h264-sdr-v1" => Ok(ReelOutputPreset::MovH264SdrV1),
        other => bail!("unsupported reel output preset {other:?}"),
    }
}

fn parse_vibe(value: &Value) -> anyhow::Result<Option<ReelVibe>> {
    match value.as_str() {
        Some("bright") => Ok(Some(ReelVibe::Bright)),
        Some("electro") => Ok(Some(ReelVibe::Electro)),
        Some("trap") => Ok(Some(ReelVibe::Trap)),
        None if value.is_null() => Ok(None),
        _ => bail!("unsupported reel vibe"),
    }
}

fn parse_format(value: &str) -> anyhow::Result<ReelFormat> {
    match value {
        "9:16" => Ok(ReelFormat::Portrait9x16),
        "4:5" => Ok(ReelFormat::Portrait4x5),
        "1:1" => Ok(ReelFormat::Square),
        "16:9" => Ok(ReelFormat::Landscape16x9),
        other => bail!("unsupported reel format {other:?}"),
    }
}

fn parse_watermark(value: &Value) -> anyhow::Result<Option<WatermarkPosition>> {
    match value.as_str() {
        Some("tl") => Ok(Some(WatermarkPosition::TopLeft)),
        Some("tr") => Ok(Some(WatermarkPosition::TopRight)),
        Some("bl") => Ok(Some(WatermarkPosition::BottomLeft)),
        Some("br") => Ok(Some(WatermarkPosition::BottomRight)),
        None if value.is_null() => Ok(None),
        _ => bail!("unsupported reel watermark"),
    }
}

fn parse_caption_position(value: &str) -> anyhow::Result<CaptionPosition> {
    match value {
        "low" => Ok(CaptionPosition::Low),
        "mid" => Ok(CaptionPosition::Mid),
        "high" => Ok(CaptionPosition::High),
        other => bail!("unsupported caption position {other:?}"),
    }
}

fn parse_transition(value: &str) -> anyhow::Result<ReelTransition> {
    match value {
        "cut" => Ok(ReelTransition::Cut),
        "mix" => Ok(ReelTransition::Mix),
        "fade" => Ok(ReelTransition::Fade),
        "white" => Ok(ReelTransition::White),
        "slideL" => Ok(ReelTransition::SlideLeft),
        "slideR" => Ok(ReelTransition::SlideRight),
        "slideU" => Ok(ReelTransition::SlideUp),
        "wipeL" => Ok(ReelTransition::WipeLeft),
        "circle" => Ok(ReelTransition::Circle),
        "blurmix" => Ok(ReelTransition::BlurMix),
        "whip" => Ok(ReelTransition::Whip),
        "zoom" => Ok(ReelTransition::Zoom),
        other => bail!("unsupported reel transition {other:?}"),
    }
}

fn parse_motion(value: &str) -> anyhow::Result<ReelMotion> {
    match value {
        "none" => Ok(ReelMotion::None),
        "in" => Ok(ReelMotion::In),
        "out" => Ok(ReelMotion::Out),
        "left" => Ok(ReelMotion::Left),
        "right" => Ok(ReelMotion::Right),
        other => bail!("unsupported reel motion {other:?}"),
    }
}

fn exact_keys(object: &Map<String, Value>, expected: &[&str], name: &str) -> anyhow::Result<()> {
    for key in expected {
        ensure!(object.contains_key(*key), "{name} is missing {key:?}");
    }
    for key in object.keys() {
        ensure!(
            expected.contains(&key.as_str()),
            "{name} contains unsupported field {key:?}"
        );
    }
    Ok(())
}

fn string<'a>(object: &'a Map<String, Value>, key: &str, name: &str) -> anyhow::Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{name} {key} must be a string"))
}

fn number(object: &Map<String, Value>, key: &str, name: &str) -> anyhow::Result<f64> {
    let value = object
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("{name} {key} must be a number"))?;
    ensure!(value.is_finite(), "{name} {key} must be finite");
    Ok(value)
}

fn nullable_string(value: &Value, name: &str) -> anyhow::Result<Option<String>> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(nonempty_owned(
        value
            .as_str()
            .with_context(|| format!("{name} must be a string"))?
            .to_owned(),
        name,
    )?))
}

fn nullable_relative_path(value: &Value, name: &str) -> anyhow::Result<Option<String>> {
    let value = nullable_string(value, name)?;
    if let Some(path) = &value {
        ensure!(
            safe_portable_relative_path(path),
            "{name} must be a safe portable relative path"
        );
    }
    Ok(value)
}

fn safe_portable_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\0')
        && !value.starts_with(['/', '\\'])
        && !value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
        && value
            .split(['/', '\\'])
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
}

fn nullable_positive_number(value: &Value, name: &str) -> anyhow::Result<Option<f64>> {
    if value.is_null() {
        return Ok(None);
    }
    let value = value
        .as_f64()
        .with_context(|| format!("{name} must be a number"))?;
    ensure!(value.is_finite() && value > 0.0, "{name} must be positive");
    Ok(Some(value))
}

fn percentage(value: &Value, name: &str) -> anyhow::Result<f64> {
    bounded(
        value
            .as_f64()
            .with_context(|| format!("{name} must be a number"))?,
        0.0,
        100.0,
        name,
    )
}

fn unit(value: f64, name: &str) -> anyhow::Result<f64> {
    bounded(value, 0.0, 1.0, name)
}

fn bounded(value: f64, minimum: f64, maximum: f64, name: &str) -> anyhow::Result<f64> {
    ensure!(
        value.is_finite() && (minimum..=maximum).contains(&value),
        "{name} is out of range"
    );
    Ok(value)
}

fn nonempty_owned(value: String, name: &str) -> anyhow::Result<String> {
    ensure!(!value.trim().is_empty(), "{name} must not be empty");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example_recipe() -> Value {
        // Frozen v2 normalization of reel-studio-main/examples/example_recipe.json. Defaults that
        // the external format permits omitting are explicit in the frozen Crush wrapper.
        json!({
            "schema_version": 2,
            "kind": "reel",
            "provenance": {"origin": "historical", "source": "reel_studio",
                           "external_id": "example_recipe.json", "profile_version": null},
            "theme": "Healthy Earth",
            "vibe": "bright",
            "music": "music/01_bright-playful/example_track_120bpm.mp3",
            "target_seconds": 30,
            "beat_snap": true,
            "format": "9:16",
            "music_volume": 100,
            "watermark": "br",
            "cover": {"id": "V1-0001_S1", "time": 2.4},
            "sequence": [
                {"id": "V1-0001_S1", "in": 1.0, "out": 4.0, "crop_x": 0.42,
                 "crop_kf": [{"t": 1.0, "x": 0.42}, {"t": 3.4, "x": 0.61}],
                 "caption": "A short warm opening line", "cap_pos": "low",
                 "transition": "mix", "speed": 1.0, "motion": "in", "clip_volume": 0,
                 "grade": {"b": 103, "c": 104, "s": 106, "t": 26, "h": 0,
                           "v": 14, "sh": 8, "hl": 0}},
                {"id": "V1-0002_S1", "in": 2.0, "out": 5.0, "crop_x": 0.34,
                 "crop_kf": [], "caption": null, "cap_pos": "low", "transition": "cut",
                 "speed": 1.0, "motion": "none", "clip_volume": 0,
                 "grade": {"b": 100, "c": 100, "s": 100, "t": 0, "h": 0,
                           "v": 0, "sh": 0, "hl": 0}}
            ],
            "crops": {"V1-0001_S1": 0.42, "V1-0002_S1": 0.34},
            "output": {"preset": "mp4-h264-sdr-v1"}
        })
    }

    #[test]
    fn parses_documented_recipe_and_resolves_exact_manual_segment_spans() {
        let recipe = parse_frozen_reel_recipe_v2(&example_recipe().to_string()).unwrap();
        assert_eq!(recipe.sequence[0].id, "V1-0001_S1");
        assert_eq!(recipe.sequence[0].segment_in_s, 1.0);
        assert_eq!(recipe.sequence[0].crop_keyframes[1].segment_time_s, 3.4);

        let segments = BTreeMap::from([
            (
                "V1-0001_S1".to_owned(),
                SegmentSourceSpan::new("video-a", "/originals/a.mov", 10.0, 20.0).unwrap(),
            ),
            (
                "V1-0002_S1".to_owned(),
                SegmentSourceSpan::new("video-b", "/originals/b.mov", 30.0, 40.0).unwrap(),
            ),
        ]);
        let resolved = resolve_reel_recipe(&recipe, &segments).unwrap();
        assert_eq!(resolved.sequence[0].source_in_s, 11.0);
        assert_eq!(resolved.sequence[0].source_out_s, 14.0);
        assert_eq!(resolved.sequence[0].item.segment_in_s, 1.0);
        assert_eq!(
            resolved.sequence[0].item.crop_keyframes[1].segment_time_s,
            3.4
        );
        assert_eq!(resolved.cover.unwrap().source_time_s, 12.4);
    }

    #[test]
    fn resolution_rejects_missing_or_too_short_segment_mappings() {
        let recipe = parse_frozen_reel_recipe_v2(&example_recipe().to_string()).unwrap();
        let missing = BTreeMap::new();
        assert!(resolve_reel_recipe(&recipe, &missing).is_err());

        let too_short = BTreeMap::from([
            (
                "V1-0001_S1".to_owned(),
                SegmentSourceSpan::new("video-a", "/originals/a.mov", 10.0, 12.0).unwrap(),
            ),
            (
                "V1-0002_S1".to_owned(),
                SegmentSourceSpan::new("video-b", "/originals/b.mov", 30.0, 40.0).unwrap(),
            ),
        ]);
        assert!(resolve_reel_recipe(&recipe, &too_short).is_err());
    }

    #[test]
    fn parser_rejects_unknown_treatment_instead_of_dropping_it() {
        let mut recipe = example_recipe();
        recipe["sequence"][0]["filter"] = json!("cinematic");
        assert!(parse_frozen_reel_recipe_v2(&recipe.to_string()).is_err());
    }

    #[test]
    fn parser_rejects_non_integer_version_and_non_portable_music_path() {
        let mut recipe = example_recipe();
        recipe["schema_version"] = json!(2.0);
        assert!(parse_frozen_reel_recipe_v2(&recipe.to_string()).is_err());

        let mut recipe = example_recipe();
        recipe["music"] = json!("../outside.mp3");
        assert!(parse_frozen_reel_recipe_v2(&recipe.to_string()).is_err());
    }
}
