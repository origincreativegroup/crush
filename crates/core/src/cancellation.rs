use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Process-local cooperative cancellation shared by Rust stages and FFmpeg children.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
