use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;

pub trait Provider {
    fn start(&self) -> ProviderHandle;
}

pub struct ProviderHandle {
    alive: Arc<AtomicBool>,
}

impl ProviderHandle {
    pub fn spawn<F>(thread_body: F) -> Self
    where
        F: FnOnce(Arc<AtomicBool>) + Send + 'static,
    {
        let alive = Arc::new(AtomicBool::new(true));
        let thread_alive = alive.clone();
        std::thread::spawn(move || thread_body(thread_alive));
        Self { alive }
    }

    pub fn stop(self) {
        self.alive.store(false, Relaxed);
    }
}
