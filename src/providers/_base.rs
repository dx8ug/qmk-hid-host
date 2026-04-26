use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;

pub trait Provider {
    fn start(&self) -> ProviderHandle;
}

pub struct ProviderHandle {
    alive: Arc<AtomicBool>,
}

impl ProviderHandle {
    pub fn new(alive: Arc<AtomicBool>) -> Self {
        Self { alive }
    }

    pub fn stop(self) {
        self.alive.store(false, Relaxed);
    }
}
