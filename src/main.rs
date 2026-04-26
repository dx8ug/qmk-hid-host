#![cfg_attr(
    all(target_os = "windows", feature = "silent", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod config;
mod data_type;
mod keyboard;
mod providers;
mod utils;

use config::load_config;
use keyboard::Keyboard;
#[cfg(target_os = "macos")]
use providers::weather::WeatherProvider;
use providers::{
    _base::{Provider, ProviderHandle},
    state::StateProvider,
    layout::LayoutProvider,
    media::MediaProvider,
    relay::RelayProvider,
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

fn main() {
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
        .from_env_lossy();
    let tracing_subscriber = tracing_subscriber::fmt().with_env_filter(env_filter).finish();
    let _ = tracing::subscriber::set_global_default(tracing_subscriber);

    let (is_connected_sender, is_connected_receiver) = mpsc::channel::<bool>(1);
    // Capacity 16: cushion for restart-cycle bursts (HELLO + immediate state from
    // ~5 providers) and pinger overlap. Lagged drops are surfaced via warn! in
    // start_write/relay/state — too small ⇒ silent packet loss.
    let (host_to_device_sender, _) = broadcast::channel::<Vec<u8>>(16);
    let (device_to_host_sender, _) = broadcast::channel::<Vec<u8>>(16);

    let args = Args::parse();
    if args.print_hids {
        return print_unique_hid_devices();
    }
    let config = load_config(args.config.unwrap_or("./qmk-hid-host.json".into()));
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
    let mut out: Vec<Box<dyn Provider>> = Vec::new();

    if enabled(&p.time) {
        out.push(TimeProvider::new(host_to_device_sender.clone()));
        tracing::info!("provider enabled: time");
    }
    if enabled(&p.volume) {
        out.push(VolumeProvider::new(host_to_device_sender.clone()));
        tracing::info!("provider enabled: volume");
    }
    if enabled(&p.layout) {
        out.push(LayoutProvider::new(host_to_device_sender.clone()));
        tracing::info!("provider enabled: layout");
    }
    if enabled(&p.media) {
        out.push(MediaProvider::new(host_to_device_sender.clone()));
        tracing::info!("provider enabled: media");
    }
    if enabled(&p.relay) {
        out.push(RelayProvider::new(host_to_device_sender.clone(), device_to_host_sender.clone()));
        tracing::info!("provider enabled: relay");
    }
    if enabled(&p.state) {
        out.push(StateProvider::new(device_to_host_sender.clone()));
        tracing::info!("provider enabled: state");
    }

    #[cfg(target_os = "macos")]
    if let Some(w) = &p.weather {
        if w.enabled {
            out.push(WeatherProvider::new(host_to_device_sender.clone(), w.url.clone()));
            tracing::info!("provider enabled: weather");
        }
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(w) = &p.weather {
        if w.enabled {
            tracing::warn!("provider 'weather' is macOS-only, ignored");
        }
    }

    out
}

fn enabled(entry: &Option<config::ProviderEntry>) -> bool {
    entry.as_ref().is_none_or(|e| e.enabled)
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
