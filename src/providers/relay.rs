use std::sync::atomic::Ordering::Relaxed;
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
        let host_to_device_sender = self.host_to_device_sender.clone();
        let mut relay_subscriber = self.device_to_host_sender.subscribe();
        ProviderHandle::spawn(move |alive| {
            // Poll via try_recv so stop() can wake the thread within IDLE_POLL on the next
            // alive check, instead of blocking until the next packet arrives.
            const IDLE_POLL: std::time::Duration = std::time::Duration::from_millis(200);
            while alive.load(Relaxed) {
                match relay_subscriber.try_recv() {
                    Ok(mut data) => {
                        if !data.is_empty() && data[0] == DataType::RelayFromDevice as u8 {
                            data[0] = DataType::RelayToDevice as u8;
                            if let Err(e) = host_to_device_sender.send(data) {
                                tracing::error!("Relay Provider failed to send data: {:?}", e);
                            }
                        }
                    }
                    Err(broadcast::error::TryRecvError::Empty) => std::thread::sleep(IDLE_POLL),
                    Err(broadcast::error::TryRecvError::Lagged(n)) => {
                        tracing::warn!("Relay Provider lagged, dropped {} packet(s)", n);
                    }
                    Err(broadcast::error::TryRecvError::Closed) => break,
                }
            }

            tracing::info!("Relay Provider stopped");
        })
    }
}
