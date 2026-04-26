use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use tokio::sync::broadcast;
use windows::Win32::{
    Globalization::{GetLocaleInfoW, LOCALE_SISO639LANGNAME},
    UI::{
        Input::KeyboardAndMouse::GetKeyboardLayout,
        TextServices::HKL,
        WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
    },
};

use crate::config::get_config;
use crate::data_type::DataType;

use super::super::_base::{Provider, ProviderHandle};

unsafe fn get_layout() -> Option<String> {
    let focused_window = GetForegroundWindow();
    let active_thread = GetWindowThreadProcessId(focused_window, Some(std::ptr::null_mut()));
    let layout = GetKeyboardLayout(active_thread);
    let locale_id = (std::mem::transmute::<HKL, u64>(layout) & 0xFFFF) as u32;
    let mut layout_name_arr = [0u16; 9];
    let _ = GetLocaleInfoW(locale_id, LOCALE_SISO639LANGNAME, Some(&mut layout_name_arr));
    if let Some(trimmed_arr) = layout_name_arr.split(|&x| x == 0u16).next() {
        return String::from_utf16(&trimmed_arr).ok();
    }

    None
}

fn send_data(value: &String, layouts: &Vec<String>, data_sender: &broadcast::Sender<Vec<u8>>) {
    if let Some(index) = layouts.into_iter().position(|r| r == value) {
        let data = vec![DataType::Layout as u8, index as u8];
        data_sender.send(data).unwrap();
    }
}

pub struct LayoutProvider {
    data_sender: broadcast::Sender<Vec<u8>>,
}

impl LayoutProvider {
    pub fn new(data_sender: broadcast::Sender<Vec<u8>>) -> Box<dyn Provider> {
        return Box::new(LayoutProvider { data_sender });
    }
}

impl Provider for LayoutProvider {
    fn start(&self) -> ProviderHandle {
        tracing::info!("Layout Provider started");
        let alive = Arc::new(AtomicBool::new(true));
        let thread_alive = alive.clone();
        let layouts = &get_config().layouts;
        let data_sender = self.data_sender.clone();
        std::thread::spawn(move || {
            let mut synced_layout = "".to_string();
            while thread_alive.load(Relaxed) {
                if let Some(layout) = unsafe { get_layout() } {
                    if synced_layout != layout {
                        synced_layout = layout;
                        send_data(&synced_layout, layouts, &data_sender);
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            tracing::info!("Layout Provider stopped");
        });
        ProviderHandle::new(alive)
    }
}
