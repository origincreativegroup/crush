//! Versioned vision prompts. The v1 prompt is ported verbatim from nodeo's
//! tuned single-call structured-JSON prompt (`LLAVA_IMPROVEMENTS.md`, implemented
//! in `app/ai/llava_client.py` `_extract_metadata_fast`). Never edit a shipped
//! prompt in place: add the next version and bump `PROMPT_VERSION`, so stored
//! descriptions stay reproducible and re-describable.

/// Version tag recorded alongside every description produced by `DESCRIBE_V1`.
pub const PROMPT_VERSION: &str = "v1";

/// Single-call structured extraction: description, tags, objects, scene, mood,
/// colors. Ported verbatim — including the fixed scene list and the examples —
/// because nodeo's temperature/token tuning was validated against this exact text.
pub const DESCRIBE_V1: &str = r#"Analyze this image in detail and provide a comprehensive analysis in JSON format.

Your response must be valid JSON with these exact keys:
{
  "description": "A detailed 2-3 sentence description covering the main subject, composition, and notable elements",
  "tags": ["array", "of", "5-10", "relevant", "lowercase", "keywords"],
  "objects": ["list", "of", "main", "visible", "objects"],
  "scene": "scene type in 1-2 words",
  "mood": "optional mood/atmosphere descriptor",
  "colors": ["dominant", "color", "palette"]
}

Guidelines:
- Description: Be specific about what makes this image unique. Include composition, subjects, and context.
- Tags: Use semantically relevant, searchable keywords (e.g., "sunset", "architecture", "portrait")
- Objects: List concrete, visible items (e.g., "person", "building", "tree", "car")
- Scene: Choose from: indoor, outdoor, portrait, landscape, urban, nature, abstract, close-up, aerial, street, studio
- Mood: Describe the atmosphere (e.g., "peaceful", "energetic", "moody", "bright")
- Colors: List 2-4 dominant colors (e.g., "blue", "warm tones", "monochrome")

Respond with valid JSON only, no additional text."#;
