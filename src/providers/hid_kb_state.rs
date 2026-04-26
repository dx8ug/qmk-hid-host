use chrono::{DateTime, Local};
use std::sync::atomic::Ordering::Relaxed;
use tokio::sync::broadcast;

use super::_base::{Provider, ProviderHandle};
use crate::data_type::{DataType, HidKbStateSubtype};

pub struct HidKbStateProvider {
    device_to_host_sender: broadcast::Sender<Vec<u8>>,
}

impl HidKbStateProvider {
    pub fn new(device_to_host_sender: broadcast::Sender<Vec<u8>>) -> Box<dyn Provider> {
        Box::new(HidKbStateProvider { device_to_host_sender })
    }
}

fn format_timestamp() -> String {
    let now: DateTime<Local> = Local::now();
    now.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

fn format_data_type(type_id: u8) -> &'static str {
    match type_id {
        x if x == DataType::Time as u8 => "Time",
        x if x == DataType::Volume as u8 => "Volume",
        x if x == DataType::Layout as u8 => "Layout",
        // 0xAD/0xAE differ across platforms (Linux: MediaArtist/MediaTitle; macOS: Spotify on 0xAE).
        0xAD => "MediaArtist",
        0xAE => "MediaTitle",
        x if x == DataType::RelayFromDevice as u8 => "RelayFromDevice",
        x if x == DataType::RelayToDevice as u8 => "RelayToDevice",
        x if x == DataType::HidKbState as u8 => "HidKbState",
        _ => "Unknown",
    }
}

fn format_hex_dump(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02X}", b)).collect::<Vec<String>>().join(" ")
}

fn parse_data_by_type(data: &[u8]) -> String {
    if data.is_empty() {
        return "Empty data".to_string();
    }

    let type_id = data[0];
    match type_id {
        x if x == DataType::Time as u8 => {
            if data.len() >= 3 {
                format!("Hour={}, Minute={}", data[1], data[2])
            } else {
                "Incomplete time data".to_string()
            }
        }
        x if x == DataType::Volume as u8 => {
            if data.len() >= 2 {
                format!("Volume={}", data[1])
            } else {
                "Incomplete volume data".to_string()
            }
        }
        x if x == DataType::Layout as u8 => {
            if data.len() >= 2 {
                format!("Layout index={}", data[1])
            } else {
                "Incomplete layout data".to_string()
            }
        }
        0xAD | 0xAE => {
            let media_type = if type_id == 0xAD { "Artist" } else { "Title" };
            if data.len() > 1 {
                let text = String::from_utf8_lossy(&data[1..]).trim_end_matches('\0').to_string();
                format!("{}=\"{}\"", media_type, text)
            } else {
                format!("Empty {} data", media_type)
            }
        }
        x if x == DataType::RelayFromDevice as u8 || x == DataType::RelayToDevice as u8 => {
            let direction = if type_id == DataType::RelayFromDevice as u8 {
                "FromDevice"
            } else {
                "ToDevice"
            };
            if data.len() > 1 {
                format!("Relay{}: {} bytes of payload", direction, data.len() - 1)
            } else {
                format!("Empty relay {} data", direction)
            }
        }
        x if x == DataType::HidKbState as u8 => {
            if data.len() >= 3 {
                let subtype = data[1];
                match subtype {
                    x if x == HidKbStateSubtype::Layer as u8 => {
                        format!("Layer={}", data[2])
                    }
                    x if x == HidKbStateSubtype::Lang as u8 => {
                        let lang_str = match data[2] {
                            0 => "(en)",
                            1 => "(ru)",
                            _ => "(unknown)",
                        };
                        format!("Lang={} {}", data[2], lang_str)
                    }
                    x if x == HidKbStateSubtype::MacMode as u8 => {
                        let mac_str = match data[2] {
                            0 => "(off)",
                            1 => "(on)",
                            _ => "(unknown)",
                        };
                        format!("MacMode={} {}", data[2], mac_str)
                    }
                    x if x == HidKbStateSubtype::RuenLayout as u8 => {
                        let layout_str = match data[2] {
                            0 => "(pc)",
                            1 => "(mac)",
                            _ => "(unknown)",
                        };
                        format!("RuenLayout={} {}", data[2], layout_str)
                    }
                    _ => {
                        format!("Unknown HidKbState subtype: {}", subtype)
                    }
                }
            } else {
                "Incomplete HidKbState".to_string()
            }
        }
        _ => {
            format!("Unknown type data: {} bytes", data.len())
        }
    }
}

impl Provider for HidKbStateProvider {
    fn start(&self) -> ProviderHandle {
        tracing::info!("HID KbState Provider started");
        let mut hid_subscriber = self.device_to_host_sender.subscribe();

        ProviderHandle::spawn(move |alive| {
            while alive.load(Relaxed) {
                tracing::debug!("HID KbState Provider: waiting for data...");
                match hid_subscriber.blocking_recv() {
                    Ok(data) => {
                        // Recheck after recv: stop() may have flipped alive while we were blocked.
                        if !alive.load(Relaxed) {
                            break;
                        }
                        let timestamp = format_timestamp();
                        let type_name = format_data_type(data[0]);
                        let hex_dump = format_hex_dump(&data);
                        let parsed_data = parse_data_by_type(&data);

                        println!("[{}] HID Data Received:", timestamp);
                        println!("  Type: {} (0x{:02X})", type_name, data[0]);
                        println!("  Size: {} bytes", data.len());
                        println!("  Raw: [{}]", hex_dump);
                        println!("  Parsed: {}", parsed_data);
                        println!();
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("HID KbState Provider lagged, dropped {} packet(s)", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            tracing::info!("HID KbState Provider stopped");
        })
    }
}
