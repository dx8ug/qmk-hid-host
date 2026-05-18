#![cfg_attr(
    all(target_os = "windows", feature = "silent", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod config;
mod data_type;
mod hid_kb_state;
mod keyboard;
mod providers;
mod utils;

use config::load_config;
use keyboard::Keyboard;
#[cfg(target_os = "macos")]
use providers::weather::WeatherProvider;
use providers::{
    _base::{Provider, ProviderHandle},
    layout::LayoutProvider,
    media::MediaProvider,
    relay::RelayProvider,
    state::StateProvider,
    streamdeck::StreamDeckBridge,
    time::TimeProvider,
    volume::VolumeProvider,
};
use tokio::sync::{broadcast, mpsc};
use utils::print_hids::print_unique_hid_devices;

#[cfg(target_os = "macos")]
use core_foundation_sys::runloop::CFRunLoopRun;

use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    /// Path to the configuration file
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,
    /// Print all connected HIDs
    #[arg(short, long)]
    print_hids: bool,
}

/// `logLevel` from config → that directive; otherwise INFO. Malformed directive → startup panic.
fn build_log_filter(config_level: Option<&str>) -> tracing_subscriber::EnvFilter {
    use tracing_subscriber::EnvFilter;

    match config_level {
        Some(level) => EnvFilter::try_new(level).unwrap_or_else(|e| panic!("Invalid logLevel '{}' in config: {}", level, e)),
        None => EnvFilter::new("info"),
    }
}

fn install_subscriber(filter: tracing_subscriber::EnvFilter) {
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter).finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

fn main() {
    let args = Args::parse();

    if args.print_hids {
        install_subscriber(build_log_filter(None));
        return print_unique_hid_devices();
    }

    let config_path = args.config.unwrap_or("./qmk-hid-host.json".into());
    let was_missing = !config_path.exists();
    let config = load_config(config_path.clone());

    install_subscriber(build_log_filter(config.log_level.as_deref()));
    if was_missing {
        tracing::info!("New config file created at {:?}", config_path);
    }

    let (is_connected_sender, is_connected_receiver) = mpsc::channel::<bool>(1);
    // Capacity 16: cushion for restart-cycle bursts (HELLO + immediate state from
    // ~5 providers) and pinger overlap. Lagged drops are surfaced via warn! in
    // start_write/relay/state — too small ⇒ silent packet loss.
    let (host_to_device_sender, _) = broadcast::channel::<Vec<u8>>(16);
    let (device_to_host_sender, _) = broadcast::channel::<Vec<u8>>(16);
    let reconnect_delay = config.reconnect_delay.unwrap_or(5000);
    for device in &config.devices {
        let host_to_device_sender = host_to_device_sender.clone();
        let device_to_host_sender = device_to_host_sender.clone();
        let is_connected_sender = is_connected_sender.clone();
        let keyboard = Keyboard::new(device, reconnect_delay);
        keyboard.connect(host_to_device_sender, device_to_host_sender, is_connected_sender);
    }

    run(host_to_device_sender, device_to_host_sender, is_connected_receiver);
}

fn get_providers(
    host_to_device_sender: &broadcast::Sender<Vec<u8>>,
    device_to_host_sender: &broadcast::Sender<Vec<u8>>,
) -> Vec<Box<dyn Provider>> {
    let p = &config::get_config().providers;
    let mut out: Vec<Box<dyn Provider>> = Vec::with_capacity(8);

    let mut try_push = |entry: &Option<config::ProviderEntry>, name: &str, make: &dyn Fn() -> Box<dyn Provider>| {
        if entry.as_ref().is_none_or(|e| e.enabled) {
            out.push(make());
            tracing::info!("provider enabled: {}", name);
        }
    };

    try_push(&p.time, "time", &|| TimeProvider::new(host_to_device_sender.clone()));
    try_push(&p.volume, "volume", &|| VolumeProvider::new(host_to_device_sender.clone()));
    try_push(&p.layout, "layout", &|| LayoutProvider::new(host_to_device_sender.clone()));
    try_push(&p.media, "media", &|| MediaProvider::new(host_to_device_sender.clone()));
    try_push(&p.relay, "relay", &|| {
        RelayProvider::new(host_to_device_sender.clone(), device_to_host_sender.clone())
    });
    try_push(&p.state, "state", &|| StateProvider::new(device_to_host_sender.clone()));

    let weather = p.weather.as_ref().filter(|w| w.enabled);
    #[cfg(target_os = "macos")]
    if let Some(w) = weather {
        out.push(WeatherProvider::new(host_to_device_sender.clone(), w.url.clone()));
        tracing::info!("provider enabled: weather");
    }
    #[cfg(not(target_os = "macos"))]
    if weather.is_some() {
        tracing::warn!("provider 'weather' is macOS-only, ignored");
    }

    let sd = p.streamdeck.as_ref().filter(|s| s.enabled);
    if let Some(s) = sd {
        out.push(StreamDeckBridge::new(device_to_host_sender.clone(), s.port));
        tracing::info!("provider enabled: streamdeck (port={})", s.port);
    }

    out
}

#[cfg(not(target_os = "macos"))]
fn run(
    host_to_device_sender: broadcast::Sender<Vec<u8>>,
    device_to_host_sender: broadcast::Sender<Vec<u8>>,
    is_connected_receiver: mpsc::Receiver<bool>,
) {
    start(host_to_device_sender, device_to_host_sender, is_connected_receiver);
}

#[cfg(target_os = "macos")]
fn run(
    host_to_device_sender: broadcast::Sender<Vec<u8>>,
    device_to_host_sender: broadcast::Sender<Vec<u8>>,
    is_connected_receiver: mpsc::Receiver<bool>,
) {
    std::thread::spawn(move || {
        start(host_to_device_sender, device_to_host_sender, is_connected_receiver);
    });
    unsafe {
        CFRunLoopRun();
    }
}

fn start(
    host_to_device_sender: broadcast::Sender<Vec<u8>>,
    device_to_host_sender: broadcast::Sender<Vec<u8>>,
    mut is_connected_receiver: mpsc::Receiver<bool>,
) {
    let providers = get_providers(&host_to_device_sender, &device_to_host_sender);

    let mut connected_count = 0;
    let mut handles: Vec<ProviderHandle> = Vec::new();

    loop {
        if let Some(is_connected) = is_connected_receiver.blocking_recv() {
            connected_count += if is_connected { 1 } else { -1 };
            tracing::info!("Connected devices: {}", connected_count);

            // if new device is connected - restart providers to send all available data
            if !handles.is_empty() && (connected_count == 0 || is_connected) {
                tracing::info!("Stopping providers");
                for handle in handles.drain(..) {
                    handle.stop();
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }

            if handles.is_empty() && connected_count > 0 {
                tracing::info!("Starting providers");
                handles = providers.iter().map(|p| p.start()).collect();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build_log_filter;

    #[test]
    fn defaults_to_info_when_config_unset() {
        assert_eq!(build_log_filter(None).to_string(), "info");
    }

    #[test]
    fn config_level_is_used() {
        assert_eq!(build_log_filter(Some("warn")).to_string(), "warn");
    }

    #[test]
    fn target_directive_parses() {
        // tracing_subscriber renders directives normalised; just assert parse succeeded.
        assert!(!build_log_filter(Some("qmk_hid_host=warn,hidapi=off")).to_string().is_empty());
    }

    #[test]
    #[should_panic(expected = "Invalid logLevel")]
    fn malformed_config_panics() {
        build_log_filter(Some("warn=bogus"));
    }
}
