//! OpenFang WebSocket multiplexed transport (`/ws`) — v1 JSON envelope.
//!
//! This endpoint is designed for high-traffic bidirectional streaming:
//! - agent run token deltas + tool I/O
//! - structured logs (audit)
//!
//! HTTP remains the CRUD/bulk plane; `/ws` carries streaming + control.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use chrono::Utc;
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tracing::info;

use crate::routes::AppState;
use crate::ws_types::{
    ControlAuthenticate, ControlErrorPayload, ControlSubscribe, ControlUnsubscribe, WsFrame,
};

const MAX_WS_PER_IP: usize = 5;

/// Per-IP connection counter.
static IP_COUNTERS: std::sync::LazyLock<DashMap<IpAddr, usize>> =
    std::sync::LazyLock::new(DashMap::new);

/// Global connection counter for metrics.
static TOTAL_CONNECTIONS: AtomicU64 = AtomicU64::new(0);

/// Per-connection state.
struct Conn {
    id: u64,
    ip: IpAddr,
    api_key: String,
    ws_cfg: openfang_types::config::WsConfig,
    authenticated: AtomicBool,
    closing: AtomicBool,
    out_queue: Mutex<Vec<WsFrame>>,
    subscriptions: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    bytes_in: AtomicUsize,
    bytes_out: AtomicUsize,
    frames_in: AtomicU64,
    frames_out: AtomicU64,
    connected_at: Instant,
}

impl Conn {
    async fn enqueue_frame(&self, frame: WsFrame) -> bool {
        if self.closing.load(Ordering::Relaxed) {
            return false;
        }
        self.out_queue.lock().await.push(frame);
        true
    }

    async fn send_error(&self, code: &str, message: &str) {
        let _ = self
            .enqueue_frame(WsFrame {
                v: 1,
                id: None,
                ts: Some(Utc::now().to_rfc3339()),
                topic: "control".to_string(),
                op: "error".to_string(),
                seq: None,
                key: None,
                payload: serde_json::to_value(ControlErrorPayload {
                    code: code.to_string(),
                    message: message.to_string(),
                    retry_after_ms: None,
                })
                .unwrap_or_default(),
            })
            .await;
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        let mut counter = IP_COUNTERS.entry(self.ip).or_insert(0);
        *counter = counter.saturating_sub(1);
        if *counter == 0 {
            drop(counter);
            IP_COUNTERS.remove(&self.ip);
        }
    }
}

/// Axum handler — upgrades HTTP to WebSocket and enters the v1 protocol loop.
pub async fn ws_v1_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let ip = addr.ip();

    if !state.kernel.config.ws.enabled {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "WebSocket v1 transport is disabled",
        )
            .into_response();
    }

    // Rate-limit per-IP connections.
    if state.kernel.config.api_key.is_empty() && !ip.is_loopback() {
        let count = IP_COUNTERS.entry(ip).or_insert(0);
        if *count >= MAX_WS_PER_IP {
            return (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "Too many WebSocket connections from this IP",
            )
                .into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_ws(socket, ip, state))
}

async fn handle_ws(socket: WebSocket, ip: IpAddr, state: Arc<AppState>) {
    let conn_id = TOTAL_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    let initial_auth = state.kernel.config.api_key.is_empty() || ip.is_loopback();

    let conn = Arc::new(Conn {
        id: conn_id,
        ip,
        api_key: state.kernel.config.api_key.clone(),
        ws_cfg: state.kernel.config.ws.clone(),
        authenticated: AtomicBool::new(initial_auth),
        closing: AtomicBool::new(false),
        out_queue: Mutex::new(Vec::new()),
        subscriptions: Mutex::new(HashMap::new()),
        bytes_in: AtomicUsize::new(0),
        bytes_out: AtomicUsize::new(0),
        frames_in: AtomicU64::new(0),
        frames_out: AtomicU64::new(0),
        connected_at: Instant::now(),
    });

    // Send welcome frame.
    let welcome = WsFrame {
        v: 1,
        id: None,
        ts: Some(Utc::now().to_rfc3339()),
        topic: "control".to_string(),
        op: "welcome".to_string(),
        seq: None,
        key: None,
        payload: serde_json::json!({
            "conn_id": conn_id,
            "authenticated": initial_auth,
            "default_credits": conn.ws_cfg.default_credits,
            "resume_ttl_ms": conn.ws_cfg.resume_ttl_ms,
            "default_codec": conn.ws_cfg.default_codec,
        }),
    };
    conn.out_queue.lock().await.push(welcome);

    info!(conn_id, %ip, "ws_v1: connected");

    let (mut sink, mut stream) = socket.split();

    // Writer task: drains out_queue and sends frames.
    let writer_conn = Arc::clone(&conn);
    let writer = tokio::spawn(async move {
        loop {
            if writer_conn.closing.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            let frames: Vec<WsFrame> = {
                let mut q = writer_conn.out_queue.lock().await;
                std::mem::take(&mut *q)
            };
            for frame in frames {
                if let Ok(json) = serde_json::to_string(&frame) {
                    let len = json.len();
                    if sink.send(Message::Text(json.into())).await.is_err() {
                        writer_conn.closing.store(true, Ordering::Relaxed);
                        break;
                    }
                    writer_conn.bytes_out.fetch_add(len, Ordering::Relaxed);
                    writer_conn.frames_out.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    });

    // Ping/heartbeat task.
    let hb_conn = Arc::clone(&conn);
    let heartbeat = tokio::spawn(async move {
        let ping_every = Duration::from_millis(hb_conn.ws_cfg.ping_interval_ms);
        let idle = Duration::from_millis(hb_conn.ws_cfg.idle_timeout_ms);
        let mut last_activity = Instant::now();
        loop {
            tokio::time::sleep(ping_every).await;
            if hb_conn.closing.load(Ordering::Relaxed) {
                break;
            }
            if last_activity.elapsed() > idle {
                info!(conn_id, "ws_v1: idle timeout — closing");
                hb_conn.closing.store(true, Ordering::Relaxed);
                break;
            }
            let _ = hb_conn
                .enqueue_frame(WsFrame {
                    v: 1,
                    id: None,
                    ts: Some(Utc::now().to_rfc3339()),
                    topic: "control".to_string(),
                    op: "ping".to_string(),
                    seq: None,
                    key: None,
                    payload: serde_json::Value::Null,
                })
                .await;
            // Update activity on any incoming frame count change.
            if hb_conn.frames_in.load(Ordering::Relaxed) > 0 {
                last_activity = Instant::now();
            }
        }
    });

    // Reader loop: process inbound frames.
    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(_) => break,
        };

        match msg {
            Message::Text(text) => {
                let bytes_in = text.len();
                conn.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
                conn.frames_in.fetch_add(1, Ordering::Relaxed);

                if bytes_in > conn.ws_cfg.max_frame_bytes {
                    conn.send_error("FRAME_TOO_LARGE", "Frame exceeds max_frame_bytes")
                        .await;
                    continue;
                }

                let frame: WsFrame = match serde_json::from_str(&text) {
                    Ok(f) => f,
                    Err(e) => {
                        conn.send_error("PARSE_ERROR", &format!("Invalid frame: {e}"))
                            .await;
                        continue;
                    }
                };

                if !conn.authenticated.load(Ordering::Relaxed) {
                    if frame.topic == "control" && frame.op == "authenticate" {
                        if let Ok(auth) =
                            serde_json::from_value::<ControlAuthenticate>(frame.payload)
                        {
                            if auth.token == conn.api_key {
                                conn.authenticated.store(true, Ordering::Relaxed);
                                let _ = conn
                                    .enqueue_frame(WsFrame {
                                        v: 1,
                                        id: None,
                                        ts: Some(Utc::now().to_rfc3339()),
                                        topic: "control".to_string(),
                                        op: "authenticated".to_string(),
                                        seq: None,
                                        key: None,
                                        payload: serde_json::Value::Null,
                                    })
                                    .await;
                            } else {
                                conn.send_error("AUTH_FAILED", "Invalid API key").await;
                            }
                        }
                    } else {
                        conn.send_error("AUTH_REQUIRED", "Authenticate first").await;
                    }
                    continue;
                }

                handle_frame(&conn, &state, frame).await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    conn.closing.store(true, Ordering::Relaxed);

    // Cleanup subscriptions.
    let subs = conn.subscriptions.lock().await;
    for (_, handle) in subs.iter() {
        handle.abort();
    }
    drop(subs);

    writer.abort();
    heartbeat.abort();

    let duration = conn.connected_at.elapsed();
    info!(
        conn_id,
        %ip,
        bytes_in = conn.bytes_in.load(Ordering::Relaxed),
        bytes_out = conn.bytes_out.load(Ordering::Relaxed),
        frames_in = conn.frames_in.load(Ordering::Relaxed),
        frames_out = conn.frames_out.load(Ordering::Relaxed),
        duration_ms = duration.as_millis() as u64,
        "ws_v1: disconnected"
    );
}

async fn handle_frame(conn: &Arc<Conn>, state: &Arc<AppState>, frame: WsFrame) {
    match (frame.topic.as_str(), frame.op.as_str()) {
        ("control", "subscribe") => {
            if let Ok(sub) = serde_json::from_value::<ControlSubscribe>(frame.payload) {
                let key = format!("{}:{}", sub.topic, conn.id);
                info!(conn_id = conn.id, topic = %sub.topic, %key, "ws_v1: subscribe");

                let handle = match sub.topic.as_str() {
                    "agent.run" => spawn_agent_run_placeholder(
                        Arc::clone(conn),
                        Arc::clone(state),
                        key.clone(),
                        sub.filter,
                    ),
                    _ => {
                        conn.send_error(
                            "UNKNOWN_TOPIC",
                            &format!("Unknown topic: {}", sub.topic),
                        )
                        .await;
                        return;
                    }
                };

                conn.subscriptions.lock().await.insert(key.clone(), handle);

                let _ = conn
                    .enqueue_frame(WsFrame {
                        v: 1,
                        id: None,
                        ts: Some(Utc::now().to_rfc3339()),
                        topic: "control".to_string(),
                        op: "subscribed".to_string(),
                        seq: None,
                        key: Some(key),
                        payload: serde_json::Value::Null,
                    })
                    .await;
            }
        }
        ("control", "unsubscribe") => {
            if let Ok(unsub) = serde_json::from_value::<ControlUnsubscribe>(frame.payload) {
                if let Some(handle) = conn.subscriptions.lock().await.remove(&unsub.key) {
                    handle.abort();
                    info!(conn_id = conn.id, key = %unsub.key, "ws_v1: unsubscribed");
                }
            }
        }
        ("control", "pong") => {
            // Client responded to ping — no-op, activity tracked by frame counter.
        }
        _ => {
            conn.send_error(
                "UNKNOWN_OP",
                &format!("Unknown {}.{}", frame.topic, frame.op),
            )
            .await;
        }
    }
}

/// Placeholder for the agent run subscription pump.
fn spawn_agent_run_placeholder(
    conn: Arc<Conn>,
    _state: Arc<AppState>,
    key: String,
    _filter: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!(key = %key, "ws_v1: agent.run subscription active (streaming not yet wired)");
        while !conn.closing.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_frame_roundtrip() {
        let frame = WsFrame {
            v: 1,
            id: Some("test-1".to_string()),
            ts: Some("2026-01-01T00:00:00Z".to_string()),
            topic: "control".to_string(),
            op: "welcome".to_string(),
            seq: None,
            key: None,
            payload: serde_json::json!({"conn_id": 42}),
        };

        let json = serde_json::to_string(&frame).unwrap();
        let parsed: WsFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.v, 1);
        assert_eq!(parsed.topic, "control");
        assert_eq!(parsed.op, "welcome");
        assert_eq!(parsed.payload["conn_id"], 42);
    }

    #[test]
    fn test_ip_counter_tracks_connections() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        IP_COUNTERS.entry(ip).and_modify(|c| *c += 1).or_insert(1);
        assert!(*IP_COUNTERS.get(&ip).unwrap() >= 1);
        IP_COUNTERS.remove(&ip);
    }
}
