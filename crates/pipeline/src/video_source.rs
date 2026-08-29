//! Production-video capability decisions kept separate from editorial processing.

use std::path::Path;

use anyhow::{bail, Context};
use crush_stage_split::ffmpeg::Probe;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "mxf", "mkv", "avi", "mts", "braw", "r3d",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyPolicy {
    pub required: bool,
    pub reason: Option<String>,
}

pub fn is_video_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

const BRAW_DECODE_REASON: &str = "BRAW decode is disabled: the Blackmagic RAW SDK has proprietary distribution and licensing requirements; embedded-preview extraction is not full media support";
const R3D_DECODE_REASON: &str = "R3D decode is disabled: RED SDK integration and redistribution licensing have not been approved; embedded-preview extraction is not full media support";

pub fn validate_decoder_policy(path: &Path) -> anyhow::Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .with_context(|| format!("video has no UTF-8 extension: {}", path.display()))?;
    match extension.as_str() {
        "braw" => bail!("{BRAW_DECODE_REASON}"),
        "r3d" => bail!("{R3D_DECODE_REASON}"),
        _ => Ok(()),
    }
}

pub fn proxy_policy(probe: &Probe) -> anyhow::Result<ProxyPolicy> {
    let codec = probe
        .video_codec
        .as_deref()
        .context("media has no video stream; it cannot enter the visual ingest pipeline")?
        .to_ascii_lowercase();
    let profile = probe
        .codec_profile
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let codec_tag = probe
        .codec_tag
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    if ["aprn", "aprh"].contains(&codec_tag.as_str())
        || ((codec.contains("prores") || profile.contains("prores"))
            && (codec.contains("raw") || profile.contains("raw")))
    {
        bail!(
            "ProRes RAW decode is disabled: the bundled LGPL FFmpeg path is not a supported full decoder and Apple SDK/licensing integration has not been approved; embedded-preview extraction is not full media support"
        );
    }

    // Extension-only gating is not enough: BRAW/R3D payloads inside allowed containers such
    // as .mov must fail with the named licensing messages instead of a generic ffmpeg error.
    if [codec.as_str(), profile.as_str(), codec_tag.as_str()]
        .iter()
        .any(|value| value.contains("braw"))
    {
        bail!("{BRAW_DECODE_REASON}");
    }
    if [codec.as_str(), profile.as_str(), codec_tag.as_str()]
        .iter()
        .any(|value| value.contains("r3d"))
    {
        bail!("{R3D_DECODE_REASON}");
    }

    if codec.contains("prores") || codec == "dnxhd" || codec == "dnxhr" {
        return Ok(ProxyPolicy {
            required: false,
            reason: None,
        });
    }
    if codec == "h264" || codec == "avc1" {
        // fps 0.0 (unparseable frame rate) and unknown bit depth are proxy-required, never
        // silently treated as cheap 8-bit/60 fps sources.
        let Some(bit_depth) = probe.bit_depth else {
            return required(format!(
                "H.264 source has an unknown bit depth (pixel format {:?}); generate a seek-friendly H.264 working proxy",
                probe.pixel_format.as_deref().unwrap_or("unknown")
            ));
        };
        if probe.fps.is_finite()
            && probe.fps > 0.0
            && probe.width <= 3840
            && probe.height <= 2160
            && probe.fps <= 60.0
            && bit_depth <= 8
        {
            return Ok(ProxyPolicy {
                required: false,
                reason: None,
            });
        }
        return required(format!(
            "H.264 source exceeds direct-edit policy ({}x{}, {:.3} fps, {}-bit)",
            probe.width, probe.height, probe.fps, bit_depth
        ));
    }
    if codec == "hevc" || codec == "h265" || codec == "hev1" || codec == "hvc1" {
        return required(format!(
            "{} acquisition media uses inter-frame HEVC/H.265; generate a seek-friendly H.264 working proxy",
            probe.codec_profile.as_deref().unwrap_or("HEVC")
        ));
    }
    required(format!(
        "codec {codec:?} is not on the measured direct-edit allowlist"
    ))
}

fn required(reason: String) -> anyhow::Result<ProxyPolicy> {
    Ok(ProxyPolicy {
        required: true,
        reason: Some(reason),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(codec: &str, profile: Option<&str>, bit_depth: u8) -> Probe {
        Probe {
            duration_s: 5.0,
            fps: 24.0,
            width: 3840,
            height: 2160,
            has_audio: true,
            container: Some("mov,mp4,m4a,3gp,3g2,mj2".to_owned()),
            video_codec: Some(codec.to_owned()),
            codec_profile: profile.map(str::to_owned),
            codec_tag: Some("avc1".to_owned()),
            pixel_format: Some(format!("yuv420p{bit_depth}le")),
            bit_depth: Some(bit_depth),
            color_space: Some("bt709".to_owned()),
            color_primaries: Some("bt709".to_owned()),
            color_transfer: Some("bt709".to_owned()),
            color_range: Some("tv".to_owned()),
            rotation: None,
        }
    }

    #[test]
    fn direct_edit_and_proxy_decisions_are_explicit() {
        assert!(
            !proxy_policy(&probe("h264", Some("High"), 8))
                .unwrap()
                .required
        );
        assert!(
            !proxy_policy(&probe("prores", Some("HQ"), 10))
                .unwrap()
                .required
        );
        assert!(
            proxy_policy(&probe("hevc", Some("Main 10"), 10))
                .unwrap()
                .required
        );
    }

    #[test]
    fn proprietary_formats_never_masquerade_as_preview_support() {
        assert!(validate_decoder_policy(Path::new("camera.braw"))
            .unwrap_err()
            .to_string()
            .contains("proprietary"));
        assert!(validate_decoder_policy(Path::new("camera.r3d"))
            .unwrap_err()
            .to_string()
            .contains("licensing"));
        assert!(proxy_policy(&probe("prores", Some("ProRes RAW HQ"), 12))
            .unwrap_err()
            .to_string()
            .contains("ProRes RAW"));
    }

    #[test]
    fn braw_and_r3d_inside_allowed_containers_fail_with_named_licensing_messages() {
        let mut mov_braw = probe("prores", Some("HQ"), 10);
        mov_braw.video_codec = Some("braw".to_owned());
        mov_braw.codec_tag = Some("BRAW".to_owned());
        assert!(proxy_policy(&mov_braw)
            .unwrap_err()
            .to_string()
            .contains("Blackmagic RAW SDK"));
        let mut mov_r3d = probe("prores", Some("HQ"), 10);
        mov_r3d.video_codec = Some("r3d".to_owned());
        mov_r3d.codec_profile = Some("REDCODE".to_owned());
        assert!(proxy_policy(&mov_r3d)
            .unwrap_err()
            .to_string()
            .contains("RED SDK"));
    }

    #[test]
    fn zero_fps_or_unknown_bit_depth_requires_a_proxy() {
        let mut zero_fps = probe("h264", Some("High"), 8);
        zero_fps.fps = 0.0;
        assert!(proxy_policy(&zero_fps).unwrap().required);
        let mut negative_fps = probe("h264", Some("High"), 8);
        negative_fps.fps = -1.0;
        assert!(proxy_policy(&negative_fps).unwrap().required);
        let mut unknown_bit_depth = probe("h264", Some("High"), 8);
        unknown_bit_depth.bit_depth = None;
        assert!(proxy_policy(&unknown_bit_depth).unwrap().required);
    }

    #[test]
    fn checked_in_matrix_records_every_production_video_decision() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/source-formats/support-matrix.json"
        ))
        .unwrap();
        let formats = fixture["videos"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["format"].as_str())
            .collect::<Vec<_>>();
        for expected in [
            "MOV/MP4/M4V/MXF",
            "ProRes",
            "H.264",
            "H.265/HEVC",
            "BRAW",
            "R3D",
            "ProRes RAW",
        ] {
            assert!(formats.contains(&expected), "missing {expected}");
        }
    }
}
