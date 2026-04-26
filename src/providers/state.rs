use chrono::Local;
use std::sync::atomic::Ordering::Relaxed;
use tokio::sync::broadcast;

use super::_base::{Provider, ProviderHandle};
use crate::data_type::{DataType, HidKbStateSubtype};

pub struct StateProvider {
    device_to_host_sender: broadcast::Sender<Vec<u8>>,
}

impl StateProvider {
    pub fn new(device_to_host_sender: broadcast::Sender<Vec<u8>>) -> Box<dyn Provider> {
        Box::new(StateProvider { device_to_host_sender })
    }
}

fn parse_kb_state(data: &[u8]) -> String {
    if data.len() < 3 {
        return "Incomplete HidKbState".to_string();
    }
    let subtype = data[1];
    let value = data[2];
    match subtype {
        x if x == HidKbStateSubtype::Layer as u8 => format!("Layer={}", value),
        x if x == HidKbStateSubtype::Lang as u8 => {
            let s = match value {
                0 => "(en)",
                1 => "(ru)",
                _ => "(unknown)",
            };
            format!("Lang={} {}", value, s)
        }
        x if x == HidKbStateSubtype::MacMode as u8 => {
            let s = match value {
                0 => "(off)",
                1 => "(on)",
                _ => "(unknown)",
            };
            format!("MacMode={} {}", value, s)
        }
        x if x == HidKbStateSubtype::RuenLayout as u8 => {
            let s = match value {
                0 => "(pc)",
                1 => "(mac)",
                _ => "(unknown)",
            };
            format!("RuenLayout={} {}", value, s)
        }
        _ => format!("Unknown HidKbState subtype: {}", subtype),
    }
}

impl Provider for StateProvider {
    fn start(&self) -> ProviderHandle {
        tracing::info!("State Provider started");
        let mut hid_subscriber = self.device_to_host_sender.subscribe();

        ProviderHandle::spawn(move |alive| {
            // Poll via try_recv so stop() can wake the thread within IDLE_POLL on the next
            // alive check, instead of blocking until the next packet arrives.
            const IDLE_POLL: std::time::Duration = std::time::Duration::from_millis(200);
            while alive.load(Relaxed) {
                match hid_subscriber.try_recv() {
                    Ok(data) => {
                        if data.is_empty() || data[0] != DataType::HidKbState as u8 {
                            continue;
                        }
                        println!(
                            "[{}] HidKbState: {}",
                            Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                            parse_kb_state(&data),
                        );
                    }
                    Err(broadcast::error::TryRecvError::Empty) => std::thread::sleep(IDLE_POLL),
                    Err(broadcast::error::TryRecvError::Lagged(n)) => {
                        tracing::warn!("State Provider lagged, dropped {} packet(s)", n);
                    }
                    Err(broadcast::error::TryRecvError::Closed) => break,
                }
            }

            tracing::info!("State Provider stopped");
        })
    }
}
