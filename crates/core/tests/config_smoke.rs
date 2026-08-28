use crush_core::Config;
use std::path::PathBuf;

#[test]
fn example_config_loads_with_documented_defaults() {
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("crush.example.toml");

    let config = Config::load(Some(&example)).expect("crush.example.toml should load");

    assert_eq!(config.split.sample_fps, 4.0);
    assert_eq!(config.split.threshold, 27.0);
    assert_eq!(config.split.min_scene_len_s, 0.6);
    assert_eq!(config.split.rep_frame_pos, 0.4);
    assert_eq!(config.embed.model, "clip-vit-b-32");
    assert_eq!(config.embed.provider, "coreml");
    assert_eq!(config.search.transcript_hit_boost, 0.15);
    assert_eq!(config.asr.model, "small");
    assert_eq!(config.asr.language, None);
    assert_eq!(config.limits.threads, 0);
    assert_eq!(config.limits.concurrent_videos, 1);
}
