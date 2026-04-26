#![cfg(target_os = "macos")]
use std::process::Command;
use std::sync::atomic::Ordering::Relaxed;
use tokio::sync::broadcast;

use crate::data_type::DataType;

use super::_base::{Provider, ProviderHandle};

fn get_weather(url: &str) -> Option<i8> {
    let output = Command::new("curl").args(["-s", url]).output();

    if let Ok(output) = output {
        if output.status.success() {
            let temp_str = String::from_utf8_lossy(&output.stdout);
            let temp_str = temp_str.trim(); // remove newline
            let temp_str = temp_str.replace(['+', '°', 'C'], "");
            if let Ok(temp) = temp_str.parse::<i8>() {
                tracing::info!("Weather Provider got temperature: {}", temp);
                return Some(temp);
            }
        }
    }
    tracing::error!("Weather Provider failed to get weather");
    None
}

fn send_data(value: &i8, host_to_device_sender: &broadcast::Sender<Vec<u8>>) {
    let data = vec![DataType::Weather as u8, *value as u8];
    if let Err(e) = host_to_device_sender.send(data) {
        tracing::error!("Weather Provider failed to send data: {:?}", e);
    }
}

pub struct WeatherProvider {
    host_to_device_sender: broadcast::Sender<Vec<u8>>,
    url: String,
}

impl WeatherProvider {
    pub fn new(host_to_device_sender: broadcast::Sender<Vec<u8>>, url: String) -> Box<dyn Provider> {
        return Box::new(WeatherProvider {
            host_to_device_sender,
            url,
        });
    }
}

impl Provider for WeatherProvider {
    fn start(&self) -> ProviderHandle {
        tracing::info!("Weather Provider started");
        let host_to_device_sender = self.host_to_device_sender.clone();
        let url = self.url.clone();
        ProviderHandle::spawn(move |alive| {
            let mut last_weather: Option<i8> = None;
            while alive.load(Relaxed) {
                if let Some(weather) = get_weather(&url) {
                    if last_weather != Some(weather) {
                        last_weather = Some(weather);
                        send_data(&weather, &host_to_device_sender);
                    }
                }

                // Update every 15 minutes
                for _ in 0..(15 * 60) {
                    if !alive.load(Relaxed) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }

            tracing::info!("Weather Provider stopped");
        })
    }
}
