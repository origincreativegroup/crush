//! Fixture-based parse tests: nodeo's JSON-repair rules against recorded
//! response shapes (clean, fenced, tags-as-string, malformed). No network.

use crush_ai::parse_description_json;

const CLEAN: &str = include_str!("fixtures/clean.json");
const FENCED: &str = include_str!("fixtures/fenced.txt");
const TAGS_STRING: &str = include_str!("fixtures/tags-string.txt");
const MALFORMED: &str = include_str!("fixtures/malformed.txt");

#[test]
fn clean_fixture_normalizes_tags() {
    let parsed = parse_description_json(CLEAN).expect("clean fixture parses");
    assert!(parsed.description.starts_with("A golden retriever"));
    // "Beach"/"beach" dedupe after lowercasing; 12 entries → 11 unique → capped at 10.
    assert_eq!(parsed.tags.len(), 10);
    assert!(
        parsed
            .tags
            .iter()
            .all(|tag| tag.chars().all(char::is_lowercase) || tag.contains(' ')),
        "tags must be lowercase: {:?}",
        parsed.tags
    );
    assert_eq!(parsed.tags.first(), Some(&"beach".to_string()));
    assert_eq!(parsed.objects, vec!["dog", "beach ball", "surf"]);
    assert_eq!(parsed.scene, "outdoor");
    assert_eq!(parsed.mood.as_deref(), Some("Energetic"));
    assert_eq!(
        parsed.colors,
        Some(vec!["Warm Tones".into(), "blue".into(), "gold".into()])
    );
}

#[test]
fn fenced_fixture_strips_markdown_fences() {
    let parsed = parse_description_json(FENCED).expect("fenced fixture parses");
    assert!(parsed.description.contains("red barn"));
    assert_eq!(
        parsed.tags,
        vec!["winter", "barn", "rural", "snow", "fence", "overcast"]
    );
    assert_eq!(parsed.scene, "rural");
    assert_eq!(parsed.mood.as_deref(), Some("quiet"));
    assert_eq!(parsed.colors, None);
}

#[test]
fn tags_as_string_fixture_splits_and_normalizes() {
    let parsed = parse_description_json(TAGS_STRING).expect("tags-as-string fixture parses");
    assert_eq!(
        parsed.tags,
        vec![
            "urban",
            "night",
            "street",
            "rain",
            "cyclist",
            "neon",
            "reflections"
        ]
    );
    // objects-as-string falls back the same way.
    assert_eq!(parsed.objects, vec!["bicycle", "traffic light"]);
    assert_eq!(parsed.scene, "urban");
    assert_eq!(parsed.mood, None);
    assert_eq!(parsed.colors, None);
}

#[test]
fn malformed_fixture_errors_with_raw_prefix() {
    let error = parse_description_json(MALFORMED).expect_err("malformed fixture must error");
    let text = error.to_string();
    assert!(text.contains("malformed JSON"), "got: {text}");
    assert!(
        text.contains("The image shows a duck"),
        "error must include the raw prefix: {text}"
    );
}
