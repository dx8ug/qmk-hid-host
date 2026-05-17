use std::sync::atomic::Ordering::Relaxed;
use tokio::sync::broadcast;

use super::_base::{Provider, ProviderHandle};
use crate::data_type::DataType;
use crate::hid_kb_state::{self, HidKbStateEvent};

pub struct StateProvider {
    device_to_host_sender: broadcast::Sender<Vec<u8>>,
}

impl StateProvider {
    pub fn new(device_to_host_sender: broadcast::Sender<Vec<u8>>) -> Box<dyn Provider> {
        Box::new(StateProvider { device_to_host_sender })
    }
}

fn format_event(event: HidKbStateEvent) -> String {
    match event {
        HidKbStateEvent::Layer(v) => format!("Layer={}", v),
        HidKbStateEvent::Lang(v) => {
            let s = match v {
                0 => "(en)",
                1 => "(ru)",
                _ => "(unknown)",
            };
            format!("Lang={} {}", v, s)
        }
        HidKbStateEvent::MacMode(v) => {
            let s = match v {
                0 => "(off)",
                1 => "(on)",
                _ => "(unknown)",
            };
            format!("MacMode={} {}", v, s)
        }
        HidKbStateEvent::RuenLayout(v) => {
            let s = match v {
                0 => "(pc)",
                1 => "(mac)",
                _ => "(unknown)",
            };
            format!("RuenLayout={} {}", v, s)
        }
    }
}

impl Provider for StateProvider {
    fn start(&self) -> ProviderHandle {
        tracing::info!("State Provider started");
        let mut hid_subscriber = self.device_to_host_sender.subscribe();

        ProviderHandle::spawn(move |alive| {
            const IDLE_POLL: std::time::Duration = std::time::Duration::from_millis(200);
            while alive.load(Relaxed) {
                match hid_subscriber.try_recv() {
                    Ok(data) => {
                        if data.is_empty() || data[0] != DataType::HidKbState as u8 {
                            continue;
                        }
                        match hid_kb_state::parse(&data) {
                            Some(event) => tracing::info!("HidKbState: {}", format_event(event)),
                            None => tracing::warn!(
                                "Unrecognised HidKbState frame (len={}, subtype={:#x})",
                                data.len(),
                                data.get(1).copied().unwrap_or(0),
                            ),
                        }
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
