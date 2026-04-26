use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::data_type::DataType;

use super::_base::{Provider, ProviderHandle};

pub struct RelayProvider {
    host_to_device_sender: broadcast::Sender<Vec<u8>>,
    device_to_host_sender: broadcast::Sender<Vec<u8>>,
}

impl RelayProvider {
    pub fn new(host_to_device_sender: broadcast::Sender<Vec<u8>>, device_to_host_sender: broadcast::Sender<Vec<u8>>) -> Box<dyn Provider> {
        return Box::new(RelayProvider {
            host_to_device_sender,
            device_to_host_sender,
        });
    }
}

impl Provider for RelayProvider {
    fn start(&self) -> ProviderHandle {
        tracing::info!("Relay Provider started");
        let alive = Arc::new(AtomicBool::new(true));
        let thread_alive = alive.clone();
        let host_to_device_sender = self.host_to_device_sender.clone();
        let mut relay_subscriber = self.device_to_host_sender.subscribe();
        std::thread::spawn(move || {
            while thread_alive.load(Relaxed) {
                tracing::debug!("Relay Provider: waiting for data...");
                if let Ok(mut data) = relay_subscriber.blocking_recv() {
                    // Recheck after recv: stop() may have flipped alive while we were blocked.
                    if !thread_alive.load(Relaxed) {
                        break;
                    }
                    // Filter only RelayFromDevice data
                    if !data.is_empty() && data[0] == DataType::RelayFromDevice as u8 {
                        data[0] = DataType::RelayToDevice as u8;
                        if let Err(e) = host_to_device_sender.send(data) {
                            tracing::error!("Relay Provider failed to send data: {:?}", e);
                        }
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            tracing::info!("Relay Provider stopped");
        });
        ProviderHandle::new(alive)
    }
}
