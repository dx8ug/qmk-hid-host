use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use super::_base::{Provider, ProviderHandle};
use crate::data_type::DataType;
use crate::hid_kb_state::{self, HidKbStateEvent};

const HELLO_FRAME: &str = r#"{"type":"hello","protocol":1,"host":"qmk-hid-host"}"#;

// Bridge is loopback-only by design: it broadcasts keyboard state without
// authentication, and the data is only intended for a local Stream Deck plugin.
const BIND_ADDR: &str = "127.0.0.1";

// Per-message write deadline. Prevents a stalled client from pinning a task
// indefinitely when the OS send buffer fills.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

// Bind retry window. On provider restart (keyboard reconnect) the previous
// bridge thread may still hold the listener for up to ~200ms while it polls
// `alive`; SO_REUSEADDR also lets us reclaim a port in TIME_WAIT.
const BIND_RETRY_DEADLINE: Duration = Duration::from_secs(5);
const BIND_RETRY_INTERVAL: Duration = Duration::from_millis(100);

type Snapshot = Arc<Mutex<HashMap<&'static str, u8>>>;

fn lock_snapshot(s: &Snapshot) -> std::sync::MutexGuard<'_, HashMap<&'static str, u8>> {
    s.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct StreamDeckBridge {
    device_to_host_sender: broadcast::Sender<Vec<u8>>,
    port: u16,
}

impl StreamDeckBridge {
    pub fn new(device_to_host_sender: broadcast::Sender<Vec<u8>>, port: u16) -> Box<dyn Provider> {
        Box::new(StreamDeckBridge {
            device_to_host_sender,
            port,
        })
    }
}

fn subtype_key(event: HidKbStateEvent) -> (&'static str, u8) {
    match event {
        HidKbStateEvent::Layer(v) => ("layer", v),
        HidKbStateEvent::Lang(v) => ("lang", v),
        HidKbStateEvent::MacMode(v) => ("macMode", v),
        HidKbStateEvent::RuenLayout(v) => ("ruenLayout", v),
    }
}

fn label_for(event: HidKbStateEvent) -> Option<&'static str> {
    match event {
        HidKbStateEvent::Layer(_) => None,
        HidKbStateEvent::Lang(0) => Some("en"),
        HidKbStateEvent::Lang(1) => Some("ru"),
        HidKbStateEvent::Lang(_) => None,
        HidKbStateEvent::MacMode(0) => Some("off"),
        HidKbStateEvent::MacMode(1) => Some("on"),
        HidKbStateEvent::MacMode(_) => None,
        HidKbStateEvent::RuenLayout(0) => Some("pc"),
        HidKbStateEvent::RuenLayout(1) => Some("mac"),
        HidKbStateEvent::RuenLayout(_) => None,
    }
}

fn state_frame(event: HidKbStateEvent) -> String {
    let (key, raw) = subtype_key(event);
    match label_for(event) {
        Some(label) => format!(r#"{{"type":"state","subtype":"{}","raw":{},"label":"{}"}}"#, key, raw, label),
        None => format!(r#"{{"type":"state","subtype":"{}","raw":{}}}"#, key, raw),
    }
}

async fn bind_with_retry(addr_str: &str, alive: &Arc<AtomicBool>) -> std::io::Result<TcpListener> {
    let addr: std::net::SocketAddr = addr_str
        .parse()
        .map_err(|e: std::net::AddrParseError| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let deadline = Instant::now() + BIND_RETRY_DEADLINE;
    let mut last_err: Option<std::io::Error> = None;
    while Instant::now() < deadline && alive.load(Relaxed) {
        let socket = if addr.is_ipv4() { TcpSocket::new_v4() } else { TcpSocket::new_v6() }?;
        socket.set_reuseaddr(true)?;
        match socket.bind(addr) {
            Ok(()) => return socket.listen(1024),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(BIND_RETRY_INTERVAL).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "bind retry exhausted")))
}

fn snapshot_frame(map: &HashMap<&'static str, u8>) -> String {
    let mut pairs: Vec<(&'static str, u8)> = map.iter().map(|(k, v)| (*k, *v)).collect();
    pairs.sort_by_key(|(k, _)| *k);
    let parts: Vec<String> = pairs.iter().map(|(k, v)| format!(r#""{}":{}"#, k, v)).collect();
    format!(r#"{{"type":"snapshot","values":{{{}}}}}"#, parts.join(","))
}

impl Provider for StreamDeckBridge {
    fn start(&self) -> ProviderHandle {
        let port = self.port;
        let hid_rx = self.device_to_host_sender.subscribe();

        tracing::info!("StreamDeck Bridge starting on {}:{}", BIND_ADDR, port);

        ProviderHandle::spawn(move |alive| {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("StreamDeck Bridge: cannot create runtime: {}", e);
                    return;
                }
            };

            rt.block_on(async move {
                let addr = format!("{}:{}", BIND_ADDR, port);
                let listener = match bind_with_retry(&addr, &alive).await {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::error!("StreamDeck Bridge: bind {} failed: {}", addr, e);
                        return;
                    }
                };
                tracing::info!("StreamDeck Bridge listening on {}", addr);

                let snapshot: Snapshot = Arc::new(Mutex::new(HashMap::new()));

                // The receiver is held only to keep the broadcast channel open
                // when no clients are subscribed; without it every send would error.
                let (frame_tx, _keep_alive_rx) = broadcast::channel::<String>(64);

                // Separate task so the snapshot stays current even before any client connects.
                let snap_for_hid = Arc::clone(&snapshot);
                let frame_tx_for_hid = frame_tx.clone();
                let alive_for_hid = Arc::clone(&alive);
                let mut hid_rx = hid_rx;
                let mut hid_alive_timer = tokio::time::interval(Duration::from_millis(200));
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = hid_alive_timer.tick() => {
                                if !alive_for_hid.load(Relaxed) { break; }
                            }
                            res = hid_rx.recv() => {
                                match res {
                                    Ok(data) => {
                                        if data.first().copied() != Some(DataType::HidKbState as u8) {
                                            continue;
                                        }
                                        if let Some(event) = hid_kb_state::parse(&data) {
                                            let (key, raw) = subtype_key(event);
                                            let frame = state_frame(event);
                                            // Holding the lock across insert + broadcast::send keeps
                                            // snapshot updates and fan-out atomic relative to the
                                            // accept arm's subscribe+snapshot block — prevents a new
                                            // client from seeing a snapshot newer than its buffered
                                            // state frames.
                                            let mut snap = lock_snapshot(&snap_for_hid);
                                            snap.insert(key, raw);
                                            // SendError means no clients are subscribed; safe to discard.
                                            let _ = frame_tx_for_hid.send(frame);
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(n)) => {
                                        tracing::warn!("StreamDeck Bridge lagged, dropped {} packet(s)", n);
                                    }
                                    Err(broadcast::error::RecvError::Closed) => break,
                                }
                            }
                        }
                    }
                });

                let alive_for_loop = Arc::clone(&alive);
                let mut alive_timer = tokio::time::interval(Duration::from_millis(200));
                loop {
                    tokio::select! {
                        _ = alive_timer.tick() => {
                            if !alive_for_loop.load(Relaxed) { break; }
                        }
                        accept_res = listener.accept() => {
                            match accept_res {
                                Ok((stream, peer)) => {
                                    tracing::info!("StreamDeck Bridge: client connected {}", peer);
                                    // Subscribe and serialize the snapshot under a single lock so
                                    // the receiver's cursor and the snapshot view are correlated.
                                    let (frame_rx, snap_text) = {
                                        let guard = lock_snapshot(&snapshot);
                                        let rx = frame_tx.subscribe();
                                        let text = snapshot_frame(&*guard);
                                        (rx, text)
                                    };
                                    tokio::spawn(handle_client(stream, peer, snap_text, frame_rx));
                                }
                                Err(e) => {
                                    tracing::warn!("StreamDeck Bridge: accept failed: {}", e);
                                }
                            }
                        }
                    }
                }
            });

            tracing::info!("StreamDeck Bridge stopped");
        })
    }
}

async fn handle_client(stream: TcpStream, peer: std::net::SocketAddr, snap_text: String, mut frame_rx: broadcast::Receiver<String>) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!("StreamDeck Bridge: handshake failed for {}: {}", peer, e);
            return;
        }
    };
    let (mut sink, mut source) = ws.split();
    match tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Text(HELLO_FRAME.to_string()))).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!("StreamDeck Bridge: send hello to {} failed: {}", peer, e);
            return;
        }
        Err(_) => {
            tracing::warn!("StreamDeck Bridge: send hello to {} timed out, dropping", peer);
            return;
        }
    }
    match tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Text(snap_text))).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!("StreamDeck Bridge: send snapshot to {} failed: {}", peer, e);
            return;
        }
        Err(_) => {
            tracing::warn!("StreamDeck Bridge: send snapshot to {} timed out, dropping", peer);
            return;
        }
    }

    loop {
        tokio::select! {
            incoming = source.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        tracing::debug!("StreamDeck Bridge: client {} read error: {}, closing", peer, e);
                        break;
                    }
                    Some(Ok(_)) => continue,
                }
            }
            frame = frame_rx.recv() => {
                match frame {
                    Ok(text) => {
                        match tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Text(text))).await {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                tracing::warn!("StreamDeck Bridge: send to {} failed, dropping: {}", peer, e);
                                break;
                            }
                            Err(_) => {
                                tracing::warn!("StreamDeck Bridge: send to {} timed out, dropping", peer);
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("StreamDeck Bridge: client {} lagged {} frames, dropping", peer, n);
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    tracing::info!("StreamDeck Bridge: client disconnected {}", peer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::sync::broadcast;
    use tokio_tungstenite::tungstenite::Message;

    async fn connect_with_retry(
        url: &str,
    ) -> Result<
        (
            tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
            tokio_tungstenite::tungstenite::handshake::client::Response,
        ),
        tokio_tungstenite::tungstenite::Error,
    > {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match tokio_tungstenite::connect_async(url).await {
                Ok(pair) => return Ok(pair),
                Err(e) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    let _ = e;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn ephemeral_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    use crate::data_type::{DataType, HidKbStateSubtype};

    fn kb_state_frame(subtype: HidKbStateSubtype, value: u8) -> Vec<u8> {
        vec![DataType::HidKbState as u8, subtype as u8, value]
    }

    #[tokio::test]
    async fn sends_snapshot_after_hello_with_cached_values() {
        let (tx, _rx) = broadcast::channel::<Vec<u8>>(16);
        let port = ephemeral_port();
        let provider = StreamDeckBridge::new(tx.clone(), port);
        let handle = provider.start();

        // Receiver is subscribed synchronously in start(), so it WILL get these.
        tx.send(kb_state_frame(HidKbStateSubtype::Layer, 2)).unwrap();
        tx.send(kb_state_frame(HidKbStateSubtype::Lang, 1)).unwrap();

        // Tiny grace period so the HID task drains both events into the cache before we connect.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut ws, _) = connect_with_retry(&url).await.expect("connect");

        // hello
        let _ = ws.next().await.expect("hello").expect("ws err");
        // snapshot
        let msg = ws.next().await.expect("snapshot").expect("ws err");
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected text, got {:?}", other),
        };
        assert!(text.contains(r#""type":"snapshot""#), "got: {}", text);
        assert!(text.contains(r#""layer":2"#), "got: {}", text);
        assert!(text.contains(r#""lang":1"#), "got: {}", text);

        let _ = ws.close(None).await;
        handle.stop();
    }

    #[tokio::test]
    async fn pushes_state_frame_after_connect() {
        let (tx, _rx) = broadcast::channel::<Vec<u8>>(16);
        let port = ephemeral_port();
        let provider = StreamDeckBridge::new(tx.clone(), port);
        let handle = provider.start();

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut ws, _) = connect_with_retry(&url).await.expect("connect");
        // discard hello + snapshot
        let _ = ws.next().await.unwrap().unwrap();
        let _ = ws.next().await.unwrap().unwrap();

        tx.send(kb_state_frame(HidKbStateSubtype::Layer, 5)).unwrap();
        let msg = ws.next().await.expect("state frame").expect("ws err");
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected text, got {:?}", other),
        };
        assert!(text.contains(r#""type":"state""#), "got: {}", text);
        assert!(text.contains(r#""subtype":"layer""#), "got: {}", text);
        assert!(text.contains(r#""raw":5"#), "got: {}", text);

        let _ = ws.close(None).await;
        handle.stop();
    }

    #[test]
    fn state_frame_includes_label_for_lang() {
        let f = state_frame(HidKbStateEvent::Lang(1));
        assert!(f.contains(r#""subtype":"lang""#), "got: {}", f);
        assert!(f.contains(r#""raw":1"#), "got: {}", f);
        assert!(f.contains(r#""label":"ru""#), "got: {}", f);
    }

    #[test]
    fn state_frame_includes_label_for_mac_mode() {
        let f = state_frame(HidKbStateEvent::MacMode(0));
        assert!(f.contains(r#""label":"off""#), "got: {}", f);
    }

    #[test]
    fn state_frame_includes_label_for_ruen_layout() {
        let f = state_frame(HidKbStateEvent::RuenLayout(1));
        assert!(f.contains(r#""label":"mac""#), "got: {}", f);
    }

    #[test]
    fn state_frame_omits_label_for_layer() {
        let f = state_frame(HidKbStateEvent::Layer(3));
        assert!(!f.contains("\"label\""), "got: {}", f);
    }

    #[test]
    fn state_frame_omits_label_for_unknown_value() {
        let f = state_frame(HidKbStateEvent::Lang(42));
        assert!(!f.contains("\"label\""), "got: {}", f);
    }

    #[tokio::test]
    async fn port_in_use_does_not_panic() {
        // Hold a listener on a port, then try to bind the provider on the same port.
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = occupied.local_addr().unwrap().port();

        let (tx, _rx) = broadcast::channel::<Vec<u8>>(16);
        let provider = StreamDeckBridge::new(tx, port);
        let handle = provider.start();

        // Give it a moment to fail bind.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        handle.stop();
        // Drop occupied listener.
        drop(occupied);
    }

    #[tokio::test]
    async fn restart_on_same_port_succeeds() {
        // Regression: provider must be able to rebind its port immediately after stop(),
        // because main.rs::start tears down and restarts all providers on every
        // keyboard reconnect with only a 200ms gap.
        let (tx, _rx) = broadcast::channel::<Vec<u8>>(16);
        let port = ephemeral_port();
        let provider = StreamDeckBridge::new(tx, port);

        let h1 = provider.start();
        let url = format!("ws://127.0.0.1:{}", port);
        let (ws1, _) = connect_with_retry(&url).await.expect("first connect");
        drop(ws1);
        h1.stop();

        // No sleep — exercise the bind race directly.
        let h2 = provider.start();
        let (ws2, _) = connect_with_retry(&url).await.expect("second connect after restart");
        drop(ws2);
        h2.stop();
    }

    #[tokio::test]
    async fn sends_hello_on_connect() {
        let (tx, _rx) = broadcast::channel::<Vec<u8>>(16);
        let port = ephemeral_port();
        let provider = StreamDeckBridge::new(tx, port);
        let handle = provider.start();

        let url = format!("ws://127.0.0.1:{}", port);
        let (mut ws, _resp) = connect_with_retry(&url).await.expect("connect");
        let msg = ws.next().await.expect("hello frame").expect("ws err");
        let text = match msg {
            Message::Text(t) => t,
            other => panic!("expected text, got {:?}", other),
        };
        assert!(text.contains(r#""type":"hello""#), "got: {}", text);
        assert!(text.contains(r#""protocol":1"#), "got: {}", text);

        let _ = ws.close(None).await;
        handle.stop();
    }
}
