//! Shared test double for crush-ai integration tests. Deterministic, no network.
//! Each test target compiles this module separately and uses a subset of it.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crush_ai::{DescribeRequest, ImageDescription, VisionProvider};

/// Deterministic provider for CI: records requests, optionally fails on a path
/// fragment, optionally sleeps while tracking observed concurrency. Plain
/// atomics/mutex fields are shared by `batch_describe` through `&self`.
pub struct FakeProvider {
    /// Describe fails for paths containing this fragment (tests per-item isolation).
    pub fail_on: Option<&'static str>,
    /// Per-call sleep so the concurrency bound is observable.
    pub delay: Option<Duration>,
    active: AtomicUsize,
    pub max_observed: AtomicUsize,
    requests: Mutex<Vec<String>>,
}

impl FakeProvider {
    pub fn new() -> Self {
        Self {
            fail_on: None,
            delay: None,
            active: AtomicUsize::new(0),
            max_observed: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn with_fail_on(mut self, fragment: &'static str) -> Self {
        self.fail_on = Some(fragment);
        self
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    pub fn recorded_requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests mutex").clone()
    }
}

impl VisionProvider for FakeProvider {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn model(&self) -> &str {
        "fake-model"
    }

    fn describe_image(&self, req: &DescribeRequest) -> anyhow::Result<ImageDescription> {
        if let Some(delay) = self.delay {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_observed.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(delay);
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
        self.requests
            .lock()
            .expect("requests mutex")
            .push(req.image_path.display().to_string());
        if let Some(fragment) = self.fail_on {
            if req.image_path.to_string_lossy().contains(fragment) {
                anyhow::bail!("fake failure for {}", req.image_path.display());
            }
        }
        Ok(ImageDescription {
            description: format!("fake description for {}", req.image_path.display()),
            tags: vec!["fake".into()],
            ..ImageDescription::default()
        })
    }
}
