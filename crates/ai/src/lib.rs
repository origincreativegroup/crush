//! crush-ai — local-first vision provider layer.
//!
//! One capability in 0.1.0: structured image description. Providers implement
//! [`VisionProvider`]; `provider = "none"` (the default) yields [`NoneProvider`],
//! whose describe call returns the standing honest capability error — never a
//! silent fallback, and nothing else in Crush is affected. The first real
//! backend is [`OllamaProvider`] over the LAN (ai-srv), speaking the plain HTTP
//! wire protocol with the workspace-pinned `ureq` (rustls). The pipeline is
//! synchronous by design, so everything here is blocking; no async runtime.
//!
//! This crate is a *service used by* later stages and app commands, not a stage
//! itself (stages read from a path and write to the store — `Stage::Describe`
//! lands in TASK-043). No image bytes are ever logged; errors carry context,
//! not payloads.

pub mod prompts;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Context;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::prompts::{DESCRIBE_V1, PROMPT_VERSION};

/// Standing honest capability error returned by [`NoneProvider`]. Exact wording
/// is product language — do not rephrase without a task.
pub const NO_PROVIDER_ERROR: &str = "AI description is not available: no vision provider is configured. Set up local Ollama in Preferences (recommended). Nothing else in Crush is affected.";

/// LLaVA generation can take a minute or more on a loaded box; the receive
/// timeout must cover a full 300-token generation, not just the connect.
const DESCRIBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DESCRIBE_RECV_TIMEOUT: Duration = Duration::from_secs(300);
/// `GET /api/tags` is a cheap local query; doctor should never hang on it.
const TAGS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TAGS_RECV_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of an unparseable model response is quoted back in the error.
const RAW_PREFIX_CHARS: usize = 200;
/// nodeo's normalization: at most 10 tags/objects and 5 colors.
const MAX_TAGS: usize = 10;
const MAX_OBJECTS: usize = 10;
const MAX_COLORS: usize = 5;

/// One vision capability, honestly reported. No capability → the method returns
/// an error, never a silent fallback.
///
/// `Send + Sync` because [`batch_describe`] shares one provider across bounded
/// std threads; both shipped providers are plain data, so this costs nothing.
pub trait VisionProvider: Send + Sync {
    /// Stable provider id, e.g. `"none"` | `"ollama"` (| `"openrouter"` in TASK-042).
    fn id(&self) -> &'static str;
    /// Model name as configured, for provenance labels.
    fn model(&self) -> &str;
    /// Structured extraction; the only AI operation in 0.1.0.
    fn describe_image(&self, req: &DescribeRequest) -> anyhow::Result<ImageDescription>;
}

/// What to describe. The provider reads the image bytes itself from the path —
/// stages hand off paths, never in-memory media.
#[derive(Debug, Clone)]
pub struct DescribeRequest {
    pub image_path: PathBuf,
    /// Version tag of the prompt used, recorded with the result for provenance.
    pub prompt_version: &'static str,
    /// nodeo's tuned finding: 0.3 for consistent JSON.
    pub temperature: f32,
    /// nodeo's tuned finding: 300 tokens covers the structured response.
    pub max_tokens: u32,
    /// Custom prompt override; the default is [`prompts::DESCRIBE_V1`]. Callers
    /// that override the prompt own its `prompt_version` label.
    pub prompt_override: Option<String>,
}

impl DescribeRequest {
    /// A request with the tuned defaults and the versioned v1 prompt.
    pub fn new(image_path: impl Into<PathBuf>) -> Self {
        Self {
            image_path: image_path.into(),
            prompt_version: PROMPT_VERSION,
            temperature: 0.3,
            max_tokens: 300,
            prompt_override: None,
        }
    }

    fn prompt(&self) -> &str {
        self.prompt_override.as_deref().unwrap_or(DESCRIBE_V1)
    }
}

/// Structured description of one image. Tags are lowercased, deduped, and
/// capped at 10 (nodeo's normalization); missing model fields become empty,
/// not errors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageDescription {
    pub description: String,
    pub tags: Vec<String>,
    pub objects: Vec<String>,
    pub scene: String,
    pub mood: Option<String>,
    pub colors: Option<Vec<String>>,
}

/// The default provider: no capability, honestly reported. Callers never need
/// `Option` plumbing — describing simply returns [`NO_PROVIDER_ERROR`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoneProvider;

impl VisionProvider for NoneProvider {
    fn id(&self) -> &'static str {
        "none"
    }

    fn model(&self) -> &str {
        "none"
    }

    fn describe_image(&self, _req: &DescribeRequest) -> anyhow::Result<ImageDescription> {
        anyhow::bail!("{NO_PROVIDER_ERROR}")
    }
}

/// Local Ollama backend (preferred path). Host is configured, never
/// auto-discovered; discovery is magic that fails confusingly.
///
/// Wire shape (nodeo's protocol, plain HTTP):
/// `POST {host}/api/chat` with
/// `{"model", "messages": [{"role": "user", "content": <prompt>, "images": ["<base64>"]}],
/// "options": {"temperature", "num_predict"}, "stream": false}` and response
/// `{"message": {"content": "..."}}`.
#[derive(Debug, Clone)]
pub struct OllamaProvider {
    host: String,
    model: String,
    agent: ureq::Agent,
}

impl OllamaProvider {
    pub fn new(host: impl Into<String>, model: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(DESCRIBE_CONNECT_TIMEOUT))
            .timeout_recv_response(Some(DESCRIBE_RECV_TIMEOUT))
            .build()
            .new_agent();
        Self {
            host: host.into(),
            model: model.into(),
            agent,
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{path}", self.host.trim_end_matches('/'))
    }

    /// Models available on the host (`GET /api/tags`). Used by the doctor
    /// provider check as evidence — never a failure when unreachable.
    pub fn list_models(&self) -> anyhow::Result<Vec<String>> {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(TAGS_CONNECT_TIMEOUT))
            .timeout_recv_response(Some(TAGS_RECV_TIMEOUT))
            .build()
            .new_agent();
        let mut response = agent
            .get(self.endpoint("api/tags"))
            .call()
            .map_err(|error| anyhow::anyhow!("Ollama at {} is unreachable ({error})", self.host))?;
        let body = response
            .body_mut()
            .read_to_string()
            .context("failed to read Ollama /api/tags response")?;
        Ok(parse_models_json(&body))
    }
}

impl VisionProvider for OllamaProvider {
    fn id(&self) -> &'static str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn describe_image(&self, req: &DescribeRequest) -> anyhow::Result<ImageDescription> {
        let image_bytes = std::fs::read(&req.image_path).with_context(|| {
            format!(
                "failed to read image for description: {}",
                req.image_path.display()
            )
        })?;
        let encoded = BASE64_STANDARD.encode(image_bytes);
        let body = chat_request_body(
            &self.model,
            req.prompt(),
            &encoded,
            req.temperature,
            req.max_tokens,
        );
        tracing::debug!(
            model = %self.model,
            path = %req.image_path.display(),
            "ollama describe request"
        );
        let mut response = self
            .agent
            .post(self.endpoint("api/chat"))
            .header("Content-Type", "application/json")
            .send(body.as_bytes())
            .map_err(|error| {
                anyhow::anyhow!(
                    "Ollama chat request failed ({error}); host {}, model {}",
                    self.host,
                    self.model
                )
            })?;
        let text = response
            .body_mut()
            .read_to_string()
            .context("failed to read Ollama chat response body")?;
        let value: serde_json::Value =
            serde_json::from_str(&text).context("Ollama returned a non-JSON chat response body")?;
        let content = value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Ollama response missing message.content (model {})",
                    self.model
                )
            })?;
        parse_description_json(content)
    }
}

/// The `/api/chat` request body, factored out so the wire shape is testable
/// without network.
fn chat_request_body(
    model: &str,
    prompt: &str,
    image_base64: &str,
    temperature: f32,
    max_tokens: u32,
) -> String {
    serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": prompt,
            "images": [image_base64],
        }],
        "options": {
            "temperature": temperature,
            "num_predict": max_tokens,
        },
        "stream": false,
    })
    .to_string()
}

/// Model names from an Ollama `/api/tags` body. Unparseable bodies yield an
/// empty list — the doctor check reports evidence, never guesses.
fn parse_models_json(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("models")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|model| {
            model
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Build the configured provider. `"none"` → [`NoneProvider`], `"ollama"` →
/// [`OllamaProvider`], anything else → an honest error naming the valid options.
pub fn provider_from_config(
    config: &crush_core::config::AiConfig,
) -> anyhow::Result<Box<dyn VisionProvider>> {
    match config.provider.as_str() {
        "none" => Ok(Box::new(NoneProvider)),
        "ollama" => Ok(Box::new(OllamaProvider::new(
            config.ollama_host.clone(),
            config.ollama_model.clone(),
        ))),
        other => {
            anyhow::bail!("unknown AI provider \"{other}\" (valid options: \"none\", \"ollama\")")
        }
    }
}

/// Parse and normalize a vision model's response content. Ports nodeo's
/// hard-won JSON robustness, fast method only: strip markdown code fences,
/// tolerate `tags` (and the other list fields) arriving as a comma-separated
/// string, lowercase + dedupe + cap tags at 10, trim strings, missing keys →
/// empty. Malformed beyond repair → `Err` including a raw prefix of the
/// response. NO hidden retries, NO legacy multi-call fallback: a malformed
/// response is a per-item honest failure the user can retry.
///
/// Public because the OpenRouter backend (TASK-042) returns the same payload
/// shape and must not grow a second parser.
pub fn parse_description_json(content: &str) -> anyhow::Result<ImageDescription> {
    let stripped = strip_code_fences(content);
    let value: serde_json::Value = serde_json::from_str(&stripped).map_err(|error| {
        anyhow::anyhow!(
            "vision model returned malformed JSON ({error}); raw prefix: {:?}",
            raw_prefix(&stripped, RAW_PREFIX_CHARS)
        )
    })?;
    let mood = value
        .get("mood")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|mood| !mood.is_empty())
        .map(str::to_string);
    let colors = normalize_string_list(value.get("colors"), false, MAX_COLORS);
    Ok(ImageDescription {
        description: value
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        tags: normalize_string_list(value.get("tags"), true, MAX_TAGS),
        objects: normalize_string_list(value.get("objects"), false, MAX_OBJECTS),
        scene: value
            .get("scene")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase(),
        mood,
        colors: (!colors.is_empty()).then_some(colors),
    })
}

/// Port of nodeo's markdown-fence handling: when the content starts with a
/// fence, drop the fence lines and keep what is between (or after) them.
fn strip_code_fences(content: &str) -> String {
    let trimmed = content.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let kept: Vec<&str> = trimmed
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect();
    kept.join("\n").trim().to_string()
}

/// First `max` characters of a malformed response, for honest error text.
fn raw_prefix(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Normalize one list-shaped model field. Accepts a JSON array or a
/// comma-separated string (nodeo's tags-as-string fallback); trims, drops
/// empties, lowercases when asked, dedupes preserving first-seen order, and
/// caps at `cap`. Missing or wrong-typed fields yield an empty list.
fn normalize_string_list(
    value: Option<&serde_json::Value>,
    lowercase: bool,
    cap: usize,
) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let raw: Vec<String> = match value {
        serde_json::Value::String(text) => text.split(',').map(str::to_string).collect(),
        serde_json::Value::Array(entries) => entries
            .iter()
            .map(|entry| match entry.as_str() {
                Some(text) => text.to_string(),
                // nodeo coerces non-string items with str(); keep that for
                // numbers, drop nulls/objects instead of stringifying them.
                None if entry.is_number() => entry.to_string(),
                None => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut items: Vec<String> = Vec::new();
    for item in raw {
        let mut item = item.trim().to_string();
        if lowercase {
            item = item.to_lowercase();
        }
        if !item.is_empty() && !items.contains(&item) {
            items.push(item);
        }
        if items.len() == cap {
            break;
        }
    }
    items
}

/// Describe many images with a bounded pool of std threads (no async). Input
/// order is preserved in the result; one item's failure never aborts the
/// batch — errors are isolated per item as strings. `max_concurrent` of 0 is
/// treated as 1.
pub fn batch_describe(
    provider: &dyn VisionProvider,
    paths: &[PathBuf],
    max_concurrent: usize,
) -> Vec<(PathBuf, Result<ImageDescription, String>)> {
    if paths.is_empty() {
        return Vec::new();
    }
    let workers = max_concurrent.max(1).min(paths.len());
    let next_index = AtomicUsize::new(0);
    let mut slots: Vec<Option<Result<ImageDescription, String>>> =
        (0..paths.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let next_index = &next_index;
        let (sender, receiver) = std::sync::mpsc::channel();
        for _ in 0..workers {
            let sender = sender.clone();
            scope.spawn(move || loop {
                let index = next_index.fetch_add(1, Ordering::SeqCst);
                if index >= paths.len() {
                    break;
                }
                let request = DescribeRequest::new(paths[index].clone());
                let outcome = provider
                    .describe_image(&request)
                    .map_err(|error| format!("{error:#}"));
                if sender.send((index, outcome)).is_err() {
                    break;
                }
            });
        }
        drop(sender);
        // Single-threaded collection: no lock needed, and the loop ends only
        // after every worker has dropped its sender.
        for (index, outcome) in receiver {
            slots[index] = Some(outcome);
        }
    });
    paths
        .iter()
        .cloned()
        .zip(slots.into_iter().map(|slot| {
            slot.unwrap_or_else(|| Err("describe worker did not report a result".to_string()))
        }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_body_matches_ollama_wire_shape() {
        let body = chat_request_body("llava", "prompt text", "QUJD", 0.3, 300);
        let value: serde_json::Value = serde_json::from_str(&body).expect("body is valid JSON");
        assert_eq!(value["model"], "llava");
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"], "prompt text");
        assert_eq!(value["messages"][0]["images"][0], "QUJD");
        assert!((value["options"]["temperature"].as_f64().expect("f64") - 0.3).abs() < 1e-6);
        assert_eq!(value["options"]["num_predict"], 300);
        assert_eq!(value["stream"], false);
    }

    #[test]
    fn parse_models_json_reads_ollama_tags_shape() {
        let models =
            parse_models_json(r#"{"models":[{"name":"llava:latest"},{"name":"qwen2.5vl:7b"}]}"#);
        assert_eq!(models, vec!["llava:latest", "qwen2.5vl:7b"]);
    }

    #[test]
    fn parse_models_json_never_guesses_on_garbage() {
        assert!(parse_models_json("not json").is_empty());
        assert!(parse_models_json("{}").is_empty());
    }

    #[test]
    fn strip_code_fences_only_touches_fenced_content() {
        assert_eq!(strip_code_fences("  plain  "), "plain");
        assert_eq!(strip_code_fences("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_code_fences("```json\n{\"a\":1}\n```\n"), "{\"a\":1}");
        // Unclosed fence: keep the payload lines (nodeo's behavior).
        assert_eq!(strip_code_fences("```\n{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn raw_prefix_respects_char_boundaries() {
        assert_eq!(raw_prefix("hello", 200), "hello");
        assert_eq!(raw_prefix("abcdef", 3), "abc");
        assert_eq!(raw_prefix("héllo wörld", 4), "héll");
    }

    #[test]
    fn missing_keys_become_empty_not_errors() {
        let parsed = parse_description_json("{}").expect("empty object parses");
        assert_eq!(parsed, ImageDescription::default());
    }

    #[test]
    fn tags_are_lowercased_deduped_and_capped() {
        let content = r#"{"tags": ["Beach", "beach", "SUNSET", "ocean", "golden hour",
            "motion", "pets", "waves", "running", "outdoors", "sand", "surf"]}"#;
        let parsed = parse_description_json(content).expect("parses");
        assert_eq!(
            parsed.tags,
            vec![
                "beach",
                "sunset",
                "ocean",
                "golden hour",
                "motion",
                "pets",
                "waves",
                "running",
                "outdoors",
                "sand"
            ]
        );
    }

    #[test]
    fn scene_is_lowercased_and_optional_fields_are_optionals() {
        let content = r#"{"scene": "  Outdoor  ", "mood": "  ", "colors": []}"#;
        let parsed = parse_description_json(content).expect("parses");
        assert_eq!(parsed.scene, "outdoor");
        assert_eq!(parsed.mood, None);
        assert_eq!(parsed.colors, None);
    }

    #[test]
    fn malformed_json_errors_with_raw_prefix() {
        let error = parse_description_json("The image shows a duck. {tags: yes")
            .expect_err("malformed content must error");
        let text = error.to_string();
        assert!(text.contains("malformed JSON"), "got: {text}");
        assert!(text.contains("The image shows a duck."), "got: {text}");
    }

    #[test]
    fn describe_request_defaults_to_versioned_v1_prompt() {
        let request = DescribeRequest::new("img.jpg");
        assert_eq!(request.prompt_version, prompts::PROMPT_VERSION);
        assert_eq!(request.prompt(), prompts::DESCRIBE_V1);
        assert!((request.temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(request.max_tokens, 300);
        let overridden = DescribeRequest {
            prompt_override: Some("custom".into()),
            ..DescribeRequest::new("img.jpg")
        };
        assert_eq!(overridden.prompt(), "custom");
    }

    #[test]
    fn none_provider_returns_the_standing_capability_error() {
        let provider = NoneProvider;
        assert_eq!(provider.id(), "none");
        let error = provider
            .describe_image(&DescribeRequest::new("img.jpg"))
            .expect_err("none provider has no capability");
        assert_eq!(error.to_string(), NO_PROVIDER_ERROR);
    }

    #[test]
    fn provider_from_config_maps_known_and_rejects_unknown() {
        let provider =
            provider_from_config(&crush_core::config::AiConfig::default()).expect("none maps");
        assert_eq!(provider.id(), "none");
        let config = crush_core::config::AiConfig {
            provider: "ollama".into(),
            ..crush_core::config::AiConfig::default()
        };
        let provider = provider_from_config(&config).expect("ollama maps");
        assert_eq!(provider.id(), "ollama");
        assert_eq!(provider.model(), "llava");
        let config = crush_core::config::AiConfig {
            provider: "openrouter".into(),
            ..crush_core::config::AiConfig::default()
        };
        let error = provider_from_config(&config)
            .err()
            .expect("unknown must error");
        let text = error.to_string();
        assert!(text.contains("openrouter"), "names the bad value: {text}");
        assert!(
            text.contains("\"none\"") && text.contains("\"ollama\""),
            "names valid options: {text}"
        );
    }

    #[test]
    fn ollama_missing_image_fails_before_any_network_call() {
        let provider = OllamaProvider::new("http://127.0.0.1:9", "llava");
        let error = provider
            .describe_image(&DescribeRequest::new("/nonexistent/path/img.jpg"))
            .expect_err("missing image must error");
        assert!(
            error.to_string().contains("failed to read image"),
            "got: {error}"
        );
    }
}
