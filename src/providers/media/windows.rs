use std::sync::atomic::Ordering::Relaxed;
use tokio::sync::broadcast;
use windows::{
    Foundation::{EventRegistrationToken, TypedEventHandler},
    Media::Control::{GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager},
};

use crate::data_type::DataType;

use super::super::_base::{Provider, ProviderHandle};

fn get_manager() -> Result<GlobalSystemMediaTransportControlsSessionManager, ()> {
    return GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|manager| manager.get())
        .map_err(|e| tracing::error!("Can not get Session Manager: {}", e));
}

fn handle_session(
    session: &GlobalSystemMediaTransportControlsSession,
    data_sender: &broadcast::Sender<Vec<u8>>,
) -> Option<EventRegistrationToken> {
    let mut synced_artist = String::new();
    let mut synced_title = String::new();
    if let Some((artist, title)) = get_media_data(session) {
        send_data(DataType::MediaArtist, &artist, &data_sender);
        send_data(DataType::MediaTitle, &title, &data_sender);
        synced_artist = artist;
        synced_title = title;
    }

    let data_sender = data_sender.clone();
    let session_handler = &TypedEventHandler::new(move |_session: &Option<GlobalSystemMediaTransportControlsSession>, _| {
        if let Some((artist, title)) = get_media_data(_session.as_ref().unwrap()) {
            if synced_artist != artist {
                send_data(DataType::MediaArtist, &artist, &data_sender);
                synced_artist = artist;
            }

            if synced_title != title {
                send_data(DataType::MediaTitle, &title, &data_sender);
                synced_title = title;
            }
        }

        Ok(())
    });

    return session
        .MediaPropertiesChanged(session_handler)
        .map_err(|e| tracing::error!("Can not register MediaPropertiesChanged callback: {}", e))
        .ok();
}

fn get_media_data(session: &GlobalSystemMediaTransportControlsSession) -> Option<(String, String)> {
    if let Ok(media_properties) = session
        .TryGetMediaPropertiesAsync()
        .and_then(|x| x.get())
        .map_err(|e| tracing::error!("Can not get media properties: {}", e))
    {
        let artist = media_properties.Artist().unwrap_or_default().to_string();
        let title = media_properties.Title().unwrap_or_default().to_string();

        if !artist.is_empty() || !title.is_empty() {
            return Some((artist, title));
        }
    }

    None
}

fn send_data(data_type: DataType, value: &String, data_sender: &broadcast::Sender<Vec<u8>>) {
    let mut data = value.to_string().into_bytes();
    data.truncate(30);
    data.insert(0, data.len() as u8);
    data.insert(0, data_type as u8);
    if let Err(e) = data_sender.send(data) {
        tracing::error!("Can not send data: {}", e);
    }

    // short delay to separate title/artist data
    std::thread::sleep(std::time::Duration::from_millis(10));
}

pub struct MediaProvider {
    data_sender: broadcast::Sender<Vec<u8>>,
}

impl MediaProvider {
    pub fn new(data_sender: broadcast::Sender<Vec<u8>>) -> Box<dyn Provider> {
        return Box::new(MediaProvider { data_sender });
    }
}

impl Provider for MediaProvider {
    fn start(&self) -> ProviderHandle {
        tracing::info!("Media Provider started");
        let data_sender = self.data_sender.clone();
        ProviderHandle::spawn(move |alive| {
            let mut session_token: Option<EventRegistrationToken> = None;

            if let Ok(manager) = get_manager() {
                if let Some(session) = manager.GetCurrentSession().ok() {
                    session_token = handle_session(&session, &data_sender);
                }

                let handler = TypedEventHandler::new(move |_manager: &Option<GlobalSystemMediaTransportControlsSessionManager>, _| {
                    if let Some(session) = _manager.as_ref().unwrap().GetCurrentSession().ok() {
                        if let Some(token) = session_token {
                            let _ = session.RemoveMediaPropertiesChanged(token);
                        }

                        session_token = handle_session(&session, &data_sender);
                    }

                    Ok(())
                });

                let manager_token = manager
                    .CurrentSessionChanged(&handler)
                    .map_err(|e| tracing::error!("Can not register CurrentSessionChanged callback: {}", e));

                while alive.load(Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }

                if let Ok(token) = manager_token {
                    let _ = manager.RemoveCurrentSessionChanged(token);
                }

                tracing::info!("Media Provider stopped");
            }
        })
    }
}
