//! Provider behavior tests: the honest capability error, config mapping, and
//! request plumbing — all without network.

mod common;

use std::path::Path;

use common::FakeProvider;
use crush_ai::{
    batch_describe, provider_from_config, DescribeRequest, NoneProvider, VisionProvider,
    NO_PROVIDER_ERROR,
};

#[test]
fn none_provider_describe_returns_the_standing_capability_error() {
    let provider = NoneProvider;
    let error = provider
        .describe_image(&DescribeRequest::new("img.jpg"))
        .expect_err("no provider means no capability");
    assert_eq!(error.to_string(), NO_PROVIDER_ERROR);
}

#[test]
fn provider_from_config_maps_none_and_ollama() {
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
}

#[test]
fn provider_from_config_rejects_unknown_naming_valid_options() {
    let config = crush_core::config::AiConfig {
        provider: "openrouter".into(),
        ..crush_core::config::AiConfig::default()
    };
    let error = provider_from_config(&config)
        .err()
        .expect("unknown provider must error");
    let text = error.to_string();
    assert!(text.contains("openrouter"), "names the bad value: {text}");
    assert!(
        text.contains("\"none\"") && text.contains("\"ollama\""),
        "names the valid options: {text}"
    );
}

#[test]
fn ollama_missing_image_fails_before_any_network_call() {
    let provider = crush_ai::OllamaProvider::new("http://127.0.0.1:9", "llava");
    let error = provider
        .describe_image(&DescribeRequest::new("/nonexistent/path/img.jpg"))
        .expect_err("missing image must error");
    assert!(
        error.to_string().contains("failed to read image"),
        "got: {error}"
    );
}

#[test]
fn fake_provider_receives_one_request_per_path() {
    let provider = FakeProvider::new();
    let paths = vec![
        Path::new("a.jpg").to_path_buf(),
        Path::new("b.jpg").to_path_buf(),
    ];
    let results = batch_describe(&provider, &paths, 2);
    // Results are order-preserving (asserted thoroughly by the batch tests);
    // the fake's internal recording order is not — workers run concurrently —
    // so compare requests as a set.
    let mut recorded = provider.recorded_requests();
    recorded.sort();
    assert_eq!(recorded, vec!["a.jpg".to_string(), "b.jpg".to_string()]);
    assert!(results.iter().all(|(_, result)| result.is_ok()));
}
