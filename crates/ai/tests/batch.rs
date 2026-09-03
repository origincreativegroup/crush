//! Batch helper behavior: order preserved, per-item errors isolated, bounded
//! concurrency. All with the deterministic fake — no network.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::FakeProvider;
use crush_ai::batch_describe;

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(Path::new).map(Path::to_path_buf).collect()
}

#[test]
fn batch_preserves_input_order() {
    let provider = FakeProvider::new();
    let inputs = paths(&["01.jpg", "02.jpg", "03.jpg", "04.jpg", "05.jpg"]);
    let results = batch_describe(&provider, &inputs, 3);
    assert_eq!(results.len(), inputs.len());
    for ((path, result), input) in results.iter().zip(&inputs) {
        assert_eq!(path, input, "input order must be preserved");
        let description = result.as_ref().expect("fake succeeds");
        assert!(description
            .description
            .contains(&input.display().to_string()));
    }
}

#[test]
fn one_bad_item_does_not_fail_the_batch() {
    let provider = FakeProvider::new().with_fail_on("bad");
    let inputs = paths(&["a.jpg", "bad.jpg", "c.jpg", "d.jpg"]);
    let results = batch_describe(&provider, &inputs, 2);
    assert_eq!(results.len(), 4);
    assert!(results[0].1.is_ok());
    let error = results[1].1.as_ref().expect_err("the bad item fails alone");
    assert!(error.contains("fake failure for"), "got: {error}");
    assert!(results[2].1.is_ok());
    assert!(results[3].1.is_ok());
}

#[test]
fn batch_respects_the_concurrency_bound() {
    let provider = FakeProvider::new().with_delay(Duration::from_millis(60));
    let inputs = paths(&["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg", "6.jpg"]);
    let results = batch_describe(&provider, &inputs, 2);
    assert_eq!(results.len(), 6);
    assert!(results.iter().all(|(_, result)| result.is_ok()));
    let observed = provider.max_observed.load(Ordering::SeqCst);
    assert!(observed <= 2, "bound must hold, observed {observed}");
    assert!(observed >= 2, "workers should overlap, observed {observed}");
}

#[test]
fn batch_treats_zero_concurrency_as_one_and_empty_as_empty() {
    let provider = FakeProvider::new();
    let inputs = paths(&["x.jpg", "y.jpg"]);
    let results = batch_describe(&provider, &inputs, 0);
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|(_, result)| result.is_ok()));

    let results = batch_describe(&provider, &[], 4);
    assert!(results.is_empty());
}
