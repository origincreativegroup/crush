//! Provider integration tests: batch wiring through a fake provider — no
//! network. The unit-level coverage (capability error, config mapping, missing
//! image, request/parsing helpers) lives in `src/lib.rs`; do not duplicate it
//! here.

mod common;

use std::path::Path;

use common::FakeProvider;
use crush_ai::batch_describe;

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
