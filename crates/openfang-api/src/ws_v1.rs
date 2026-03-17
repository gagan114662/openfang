//! OpenFang WebSocket multiplexed transport (`/ws`) — v1 JSON/MessagePack envelope.
//!
//! This endpoint is designed for high-traffic bidirectional streaming:
//! - agent run token deltas + tool I/O
//! - Nitro events + txns
//! - structured logs (audit)
//!
//! HTTP remains the CRUD/bulk plane; `/ws` carries streaming + control.

use std::collections::{HashMap, VecDeque};
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
use openfang_kernel::nitro::NitroComputerManager;
use openfang_runtime::kernel_handle::KernelHandle;
use openfang_runtime::llm_driver::StreamEvent;
use openfang_types::agent::AgentId;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::routes::AppState;
use crate::ws_types::{
    AgentRunStart, ControlAck, ControlAuthenticate, ControlErrorPayload, ControlResume,
    ControlSubscribe, ControlUnsubscribe, CreditsGrant, ResumeStream, ToolExecStart, WsFrame,
};

const MAX_WS_PER_IP: usize = 5;
const MAX_PENDING_FRAMES_PER_STREAM: usize = 512;
const MAX_RING_FRAMES_PER_STREAM: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    Json,
    MsgPack,
}

impl Codec {
    fn from_first_message(m: &Message) -> Option<Self> {
        match m {
            Message::Text(_) => Some(Self::Json),
            Message::Binary(_) => Some(Self::MsgPack),
            _ => None,
        }
    }
}

#[derive(Default)]
struct WsMetrics {
    connections: AtomicUsize,
    frames_in: AtomicUsize,
    frames_out: AtomicUsize,
    bytes_in: AtomicUsize,
    bytes_out: AtomicUsize,
    resume_ok: AtomicUsize,
    resume_fail: AtomicUsize,
    overflow: AtomicUsize,
    auth_fail: AtomicUsize,
}

static METRICS: std::sync::LazyLock<WsMetrics> = std::sync::LazyLock::new(WsMetrics::default);

pub fn metrics_snapshot() -> HashMap<&'static str, usize> {
    let m = &*METRICS;
    HashMap::from([
        ("of_ws_connections", m.connections.load(Ordering::Relaxed)),
        ("of_ws_frames_in_total", m.frames_in.load(Ordering::Relaxed)),
        ("of_ws_frames_out_total", m.frames_out.load(Ordering::Relaxed)),
        ("of_ws_bytes_in_total", m.bytes_in.load(Ordering::Relaxed)),
        ("of_ws_bytes_out_total", m.bytes_out.load(Ordering::Relaxed)),
        ("of_ws_resume_success_total", m.resume_ok.load(Ordering::Relaxed)),
        ("of_ws_resume_fail_total", m.resume_fail.load(Ordering::Relaxed)),
        ("of_ws_overflow_total", m.overflow.load(Ordering::Relaxed)),
        ("of_ws_auth_fail_total", m.auth_fail.load(Ordering::Relaxed)),
    ])
}

static WS_IP_COUNTS: std::sync::LazyLock<DashMap<IpAddr, AtomicUsize>> =
    std::sync::LazyLock::new(DashMap::new);

/// Global resume store: keyed by stream `key`, holding a TTL-bounded ring buffer.
///
/// This enables reconnect + `control.resume` to work across connections.
static RESUME_STORE: std::sync::LazyLock<DashMap<String, std::sync::Mutex<VecDeque<RingFrame>>>> =
    std::sync::LazyLock::new(DashMap::new);

fn global_ring_push(key: &str, rf: RingFrame, ttl: Duration) {
    if key == "control" {
        return;
    }
    let entry = RESUME_STORE
        .entry(key.to_string())
        .or_insert_with(|| std::sync::Mutex::new(VecDeque::new()));

    let mut ring = entry.lock().unwrap_or_else(|e| e.into_inner());
    while let Some(front) = ring.front() {
        if front.at.elapsed() > ttl {
            ring.pop_front();
        } else {
            break;
        }
    }
    if ring.len() >= MAX_RING_FRAMES_PER_STREAM {
        ring.pop_front();
    }
    ring.push_back(rf);
}

fn global_ring_snapshot(
    key: &str,
    last_seq: u64,
    ttl: Duration,
) -> Option<(Vec<WsFrame>, u64, u64)> {
    let entry = RESUME_STORE.get(key)?;
    let ring = entry.lock().unwrap_or_else(|e| e.into_inner());
    let mut frames: Vec<WsFrame> = Vec::new();
    let mut latest_seq: u64 = 0;
    let mut oldest_seq: u64 = 0;

    for rf in ring.iter() {
        if rf.at.elapsed() > ttl {
            continue;
        }
        if oldest_seq == 0 {
            oldest_seq = rf.seq;
        }
        latest_seq = latest_seq.max(rf.seq);
        if rf.seq > last_seq {
            frames.push(rf.frame.clone());
        }
    }

    Some((frames, latest_seq, oldest_seq))
}

/// GET /ws — multiplexed WS transport (v1).
pub async fn ws_v1(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    // Feature gate: disabled by default.
    if !state.kernel.config.ws.enabled {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }

    // SECURITY: match HTTP auth behavior: when api_key is empty, restrict to loopback.
    let ip = addr.ip();
    if state.kernel.config.api_key.is_empty() && !ip.is_loopback() {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }

    // Per-IP connection cap.
    let entry = WS_IP_COUNTS.entry(ip).or_insert_with(|| AtomicUsize::new(0));
    let current = entry.value().fetch_add(1, Ordering::Relaxed);
    if current >= MAX_WS_PER_IP {
        entry.value().fetch_sub(1, Ordering::Relaxed);
        return axum::http::StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    // Try header/query auth. If it fails, the client can still `control.authenticate`.
    let api_key = state.kernel.config.api_key.clone();
    let initial_auth = auth_ok(&api_key, &headers, &uri);

    // Subprotocol negotiation (best-effort). We still auto-detect codec from the first message.
    let ws = ws.protocols(["of-ws.v1.json", "of-ws.v1.msgpack"]);
    ws.on_upgrade(move |socket| conn_handle(socket, state, ip, initial_auth)).into_response()
}

fn auth_ok(api_key: &str, headers: &axum::http::HeaderMap, uri: &axum::http::Uri) -> bool {
    if api_key.is_empty() {
        return true;
    }

    let header_token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    let query_token = uri
        .query()
        .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("token=")));

    let mut ok = false;
    for token in [header_token, query_token].into_iter().flatten() {
        use subtle::ConstantTimeEq;
        if token.len() == api_key.len() && bool::from(token.as_bytes().ct_eq(api_key.as_bytes()))
        {
            ok = true;
        }
    }
    ok
}

#[derive(Clone)]
struct RingFrame {
    seq: u64,
    frame: WsFrame,
    at: Instant,
}

struct StreamBuf {
    credits: i64,
    acked_seq: u64,
    ring: VecDeque<RingFrame>,
    pending: VecDeque<WsFrame>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl StreamBuf {
    fn new(credits: i64) -> Self {
        Self {
            credits,
            acked_seq: 0,
            ring: VecDeque::new(),
            pending: VecDeque::new(),
            task: None,
        }
    }

    fn ring_push(&mut self, seq: u64, frame: WsFrame, at: Instant) {
        if self.ring.len() >= MAX_RING_FRAMES_PER_STREAM {
            self.ring.pop_front();
        }
        self.ring.push_back(RingFrame {
            seq,
            frame,
            at,
        });
    }

    fn ring_replay_since(&self, since: u64, ttl: Duration) -> Vec<WsFrame> {
        self.ring
            .iter()
            .filter(|rf| rf.seq > since && rf.at.elapsed() <= ttl)
            .map(|rf| rf.frame.clone())
            .collect()
    }

    fn ack_prune(&mut self, last_seq: u64) {
        self.acked_seq = self.acked_seq.max(last_seq);
        while let Some(rf) = self.ring.front() {
            if rf.seq <= self.acked_seq {
                self.ring.pop_front();
            } else {
                break;
            }
        }
    }
}

struct Conn {
    conn_id: String,
    opened_at: Instant,
    initial_auth: bool,
    api_key: String,
    ws_cfg: openfang_types::config::WsConfig,
    authenticated: AtomicBool,
    next_seq: AtomicU64,
    codec: Mutex<Option<Codec>>,
    last_pong: Mutex<Instant>,
    streams: Mutex<HashMap<String, StreamBuf>>,
    out_tx: tokio::sync::mpsc::Sender<Message>,
    closing: AtomicBool,
    frames_in: AtomicUsize,
    frames_out: AtomicUsize,
    bytes_in: AtomicUsize,
    bytes_out: AtomicUsize,
}

impl Conn {
    async fn set_codec_if_unset(&self, m: &Message) {
        let mut c = self.codec.lock().await;
        if c.is_none() {
            *c = Codec::from_first_message(m);
        }
    }

    async fn encode_frame(&self, frame: &WsFrame) -> Result<Message, String> {
        let codec = self.codec.lock().await.unwrap_or(Codec::Json);
        match codec {
            Codec::Json => serde_json::to_string(frame)
                .map(|s| Message::Text(s.into()))
                .map_err(|e| format!("Failed to encode WS frame (json): {e}")),
            Codec::MsgPack => rmp_serde::to_vec_named(frame)
                .map(|v| Message::Binary(v.into()))
                .map_err(|e| format!("Failed to encode WS frame (msgpack): {e}")),
        }
    }

    async fn decode_frame(&self, m: &Message) -> Result<WsFrame, String> {
        let codec = self.codec.lock().await.unwrap_or(Codec::Json);
        match (codec, m) {
            (Codec::Json, Message::Text(t)) => {
                serde_json::from_str(t).map_err(|e| format!("Invalid JSON frame: {e}"))
            }
            (Codec::MsgPack, Message::Binary(b)) => rmp_serde::from_slice(b)
                .map_err(|e| format!("Invalid MessagePack frame: {e}")),
            // Allow cross-mode fallback for easier clients: try both.
            (_, Message::Text(t)) => serde_json::from_str(t)
                .map_err(|e| format!("Invalid JSON frame: {e}")),
            (_, Message::Binary(b)) => rmp_serde::from_slice(b)
                .map_err(|e| format!("Invalid MessagePack frame: {e}")),
            _ => Err("Unsupported WS message type".to_string()),
        }
    }

    fn alloc_seq(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_control_error(&self, code: &str, message: &str, retry_after_ms: Option<u64>) {
        let payload = ControlErrorPayload {
            code: code.to_string(),
            message: message.to_string(),
            retry_after_ms,
        };
        let frame = WsFrame {
            v: 1,
            id: None,
            ts: Some(Utc::now().to_rfc3339()),
            topic: "control".to_string(),
            op: "error".to_string(),
            seq: Some(self.alloc_seq()),
            key: None,
            payload: serde_json::to_value(payload).unwrap_or_else(|_| serde_json::json!({})),
        };
        if let Ok(msg) = self.encode_frame(&frame).await {
            let _ = self.out_tx.send(msg).await;
        }
    }

    async fn enqueue_frame(&self, mut frame: WsFrame) -> Result<(), String> {
        // Normalize envelope.
        frame.v = 1;
        frame.ts = Some(Utc::now().to_rfc3339());
        let seq = self.alloc_seq();
        frame.seq = Some(seq);
        let key = frame.key.clone().unwrap_or_else(|| "control".to_string());
        let at = Instant::now();
        let ttl = Duration::from_millis(self.ws_cfg.resume_ttl_ms);

        // If the socket is closing, still capture frames for resumability,
        // but don't attempt to buffer/send on the dead connection.
        if self.closing.load(Ordering::Relaxed) {
            global_ring_push(&key, RingFrame { seq, frame, at }, ttl);
            return Ok(());
        }

        // Decide send vs buffer without holding locks across await.
        let mut to_send: Vec<WsFrame> = Vec::new();
        {
            let mut streams = self.streams.lock().await;
            let sb = streams
                .entry(key.clone())
                .or_insert_with(|| StreamBuf::new(self.ws_cfg.default_credits as i64));

            sb.ring_push(seq, frame.clone(), at);
            global_ring_push(&key, RingFrame { seq, frame: frame.clone(), at }, ttl);

            if key == "control" {
                to_send.push(frame);
            } else if sb.credits > 0 {
                sb.credits -= 1;
                to_send.push(frame);
            } else {
                if sb.pending.len() >= MAX_PENDING_FRAMES_PER_STREAM {
                    METRICS.overflow.fetch_add(1, Ordering::Relaxed);
                    sb.pending.pop_front();
                    // Non-fatal: keep the connection, but notify overflow.
                    drop(streams);
                    self.send_control_error(
                        "overflow",
                        "stream queue overflow (dropped oldest frame); client should resume or refetch snapshot",
                        None,
                    )
                    .await;
                    return Ok(());
                }
                sb.pending.push_back(frame);
            }
        }

        for f in to_send {
            let msg = self.encode_frame(&f).await?;
            let bytes = match &msg {
                Message::Text(t) => t.len(),
                Message::Binary(b) => b.len(),
                _ => 0,
            };
            METRICS.frames_out.fetch_add(1, Ordering::Relaxed);
            METRICS.bytes_out.fetch_add(bytes, Ordering::Relaxed);
            self.frames_out.fetch_add(1, Ordering::Relaxed);
            self.bytes_out.fetch_add(bytes, Ordering::Relaxed);
            let _ = self.out_tx.send(msg).await;
        }

        Ok(())
    }

    async fn enqueue_existing_frame(&self, frame: WsFrame) -> Result<(), String> {
        if self.closing.load(Ordering::Relaxed) {
            return Ok(());
        }
        let key = frame.key.clone().unwrap_or_else(|| "control".to_string());

        let mut to_send: Vec<WsFrame> = Vec::new();
        {
            let mut streams = self.streams.lock().await;
            let sb = streams
                .entry(key.clone())
                .or_insert_with(|| StreamBuf::new(self.ws_cfg.default_credits as i64));

            if key == "control" {
                to_send.push(frame);
            } else if sb.credits > 0 {
                sb.credits -= 1;
                to_send.push(frame);
            } else {
                if sb.pending.len() >= MAX_PENDING_FRAMES_PER_STREAM {
                    METRICS.overflow.fetch_add(1, Ordering::Relaxed);
                    sb.pending.pop_front();
                    drop(streams);
                    self.send_control_error(
                        "overflow",
                        "stream queue overflow (dropped oldest frame); client should resume or refetch snapshot",
                        None,
                    )
                    .await;
                    return Ok(());
                }
                sb.pending.push_back(frame);
            }
        }

        for f in to_send {
            let msg = self.encode_frame(&f).await?;
            let bytes = match &msg {
                Message::Text(t) => t.len(),
                Message::Binary(b) => b.len(),
                _ => 0,
            };
            METRICS.frames_out.fetch_add(1, Ordering::Relaxed);
            METRICS.bytes_out.fetch_add(bytes, Ordering::Relaxed);
            self.frames_out.fetch_add(1, Ordering::Relaxed);
            self.bytes_out.fetch_add(bytes, Ordering::Relaxed);
            let _ = self.out_tx.send(msg).await;
        }

        Ok(())
    }

    async fn grant_credits_and_flush(&self, key: &str, grant: i64) {
        let mut to_send = Vec::new();
        {
            let mut streams = self.streams.lock().await;
            if let Some(sb) = streams.get_mut(key) {
                sb.credits = sb.credits.saturating_add(grant);
                while sb.credits > 0 {
                    let Some(f) = sb.pending.pop_front() else { break };
                    sb.credits -= 1;
                    to_send.push(f);
                }
            }
        }

        for f in to_send {
            if let Ok(msg) = self.encode_frame(&f).await {
                let bytes = match &msg {
                    Message::Text(t) => t.len(),
                    Message::Binary(b) => b.len(),
                    _ => 0,
                };
                METRICS.frames_out.fetch_add(1, Ordering::Relaxed);
                METRICS.bytes_out.fetch_add(bytes, Ordering::Relaxed);
                self.frames_out.fetch_add(1, Ordering::Relaxed);
                self.bytes_out.fetch_add(bytes, Ordering::Relaxed);
                let _ = self.out_tx.send(msg).await;
            }
        }
    }

    async fn ack_and_prune(&self, key: &str, last_seq: u64) {
        let mut streams = self.streams.lock().await;
        if let Some(sb) = streams.get_mut(key) {
            sb.ack_prune(last_seq);
        }
    }
}

async fn conn_handle(socket: WebSocket, state: Arc<AppState>, ip: IpAddr, initial_auth: bool) {
    METRICS.connections.fetch_add(1, Ordering::Relaxed);
    let selected_protocol = socket
        .protocol()
        .and_then(|p| p.to_str().ok())
        .map(str::to_string);
    let span = tracing::info_span!(
        "of_ws_v1",
        ip = %ip,
        protocol = selected_protocol.as_deref().unwrap_or("")
    );
    let _enter = span.enter();

    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(512);

    let conn = Arc::new(Conn {
        conn_id: uuid::Uuid::new_v4().to_string(),
        opened_at: Instant::now(),
        initial_auth,
        api_key: state.kernel.config.api_key.clone(),
        ws_cfg: state.kernel.config.ws.clone(),
        authenticated: AtomicBool::new(initial_auth || state.kernel.config.api_key.is_empty()),
        next_seq: AtomicU64::new(1),
        codec: Mutex::new(None),
        last_pong: Mutex::new(Instant::now()),
        streams: Mutex::new(HashMap::new()),
        out_tx,
        closing: AtomicBool::new(false),
        frames_in: AtomicUsize::new(0),
        frames_out: AtomicUsize::new(0),
        bytes_in: AtomicUsize::new(0),
        bytes_out: AtomicUsize::new(0),
    });

    // If the client negotiated a known subprotocol, prefer it over auto-detect.
    if let Some(proto) = selected_protocol.as_deref() {
        let mut c = conn.codec.lock().await;
        if proto == "of-ws.v1.msgpack" {
            *c = Some(Codec::MsgPack);
        } else if proto == "of-ws.v1.json" {
            *c = Some(Codec::Json);
        }
    } else {
        // No negotiated protocol: use config default (still allows auto-detect on first message).
        let mut c = conn.codec.lock().await;
        if c.is_none() {
            let d = conn.ws_cfg.default_codec.to_lowercase();
            if d == "msgpack" {
                *c = Some(Codec::MsgPack);
            } else {
                *c = Some(Codec::Json);
            }
        }
    }

    // Outbound send loop (single writer).
    let send_conn = Arc::clone(&conn);
    let send_handle = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
        send_conn.closing.store(true, Ordering::Relaxed);
    });

    // Heartbeat ping + idle timeout (transport-level ping/pong).
    let hb_conn = Arc::clone(&conn);
    tokio::spawn(async move {
        let ping_every = Duration::from_millis(hb_conn.ws_cfg.ping_interval_ms);
        let idle = Duration::from_millis(hb_conn.ws_cfg.idle_timeout_ms);
        let mut interval = tokio::time::interval(ping_every);
        loop {
            interval.tick().await;
            if hb_conn.closing.load(Ordering::Relaxed) {
                break;
            }
            let last = *hb_conn.last_pong.lock().await;
            if last.elapsed() >= idle {
                hb_conn
                    .send_control_error("idle_timeout", "WS idle timeout", None)
                    .await;
                hb_conn.closing.store(true, Ordering::Relaxed);
                break;
            }
            let _ = hb_conn.out_tx.send(Message::Ping(vec![].into())).await;
        }
    });

    // Initial hello.
    let _ = conn
        .enqueue_frame(WsFrame {
            v: 1,
            id: None,
            ts: None,
            topic: "control".to_string(),
            op: "hello".to_string(),
            seq: None,
            key: None,
            payload: serde_json::json!({
                "v": 1,
                "default_credits": conn.ws_cfg.default_credits,
                "resume_ttl_ms": conn.ws_cfg.resume_ttl_ms,
                "default_codec": conn.ws_cfg.default_codec,
            }),
        })
        .await;

    // Main receive loop.
    while let Some(Ok(m)) = receiver.next().await {
        if conn.closing.load(Ordering::Relaxed) {
            break;
        }

        // Update codec from first meaningful message (Text/Binary).
        conn.set_codec_if_unset(&m).await;

        let bytes_in = match &m {
            Message::Text(t) => t.len(),
            Message::Binary(b) => b.len(),
            _ => 0,
        };
        METRICS.frames_in.fetch_add(1, Ordering::Relaxed);
        METRICS.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
        conn.frames_in.fetch_add(1, Ordering::Relaxed);
        conn.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);

        if bytes_in > conn.ws_cfg.max_frame_bytes {
            conn.send_control_error("bad_request", "frame too large", None)
                .await;
            conn.closing.store(true, Ordering::Relaxed);
            break;
        }

        match &m {
            Message::Pong(_) => {
                *conn.last_pong.lock().await = Instant::now();
                continue;
            }
            Message::Ping(d) => {
                let _ = conn.out_tx.send(Message::Pong(d.clone())).await;
                continue;
            }
            Message::Close(_) => break,
            Message::Text(_) | Message::Binary(_) => {}
        }

        let frame = match conn.decode_frame(&m).await {
            Ok(f) => f,
            Err(e) => {
                conn.send_control_error("bad_request", &e, None).await;
                continue;
            }
        };

        // Enforce auth for all non-control traffic.
        if !conn.authenticated.load(Ordering::Relaxed)
            && (frame.topic != "control" || frame.op != "authenticate")
        {
            METRICS.auth_fail.fetch_add(1, Ordering::Relaxed);
            conn.send_control_error("auth_failed", "not authenticated", None)
                .await;
            continue;
        }

        if let Err(e) = handle_incoming(&conn, &state, frame).await {
            warn!(err = %e, "WS handler error");
            conn.send_control_error("internal", &e, None).await;
        }
    }

    conn.closing.store(true, Ordering::Relaxed);
    send_handle.abort();

    let (streams_active, topics): (usize, Vec<String>) = {
        let streams = conn.streams.lock().await;
        let mut topics: Vec<String> = streams
            .keys()
            .map(|k| k.split(':').next().unwrap_or("unknown").to_string())
            .collect();
        topics.sort();
        topics.dedup();
        (streams.len(), topics)
    };
    let duration_ms = conn.opened_at.elapsed().as_millis() as u64;
    let codec = match *conn.codec.lock().await {
        Some(Codec::Json) => "json",
        Some(Codec::MsgPack) => "msgpack",
        None => "unknown",
    };

    // Wide event: one structured connection summary on close.
    info!(
        target = "openfang_ws",
        event = "ws.connection",
        conn_id = %conn.conn_id,
        ip = %ip,
        protocol = selected_protocol.as_deref().unwrap_or(""),
        codec,
        authenticated = conn.authenticated.load(Ordering::Relaxed),
        initial_auth = conn.initial_auth,
        duration_ms,
        streams_active,
        topics = ?topics,
        frames_in = conn.frames_in.load(Ordering::Relaxed),
        bytes_in = conn.bytes_in.load(Ordering::Relaxed),
        frames_out = conn.frames_out.load(Ordering::Relaxed),
        bytes_out = conn.bytes_out.load(Ordering::Relaxed),
    );

    if let Some(ent) = WS_IP_COUNTS.get(&ip) {
        let prev = ent.value().fetch_sub(1, Ordering::Relaxed);
        if prev <= 1 {
            drop(ent);
            WS_IP_COUNTS.remove(&ip);
        }
    }
    METRICS.connections.fetch_sub(1, Ordering::Relaxed);
}

async fn handle_incoming(conn: &Arc<Conn>, state: &Arc<AppState>, frame: WsFrame) -> Result<(), String> {
    match (frame.topic.as_str(), frame.op.as_str()) {
        ("control", "authenticate") => {
            let req: ControlAuthenticate = serde_json::from_value(frame.payload)
                .map_err(|e| format!("Invalid authenticate payload: {e}"))?;
            if !auth_token_ok(&conn.api_key, &req.token) {
                METRICS.auth_fail.fetch_add(1, Ordering::Relaxed);
                conn.send_control_error("auth_failed", "invalid token", Some(5_000))
                    .await;
                return Ok(());
            }
            conn.authenticated.store(true, Ordering::Relaxed);
            conn.enqueue_frame(WsFrame {
                v: 1,
                id: frame.id,
                ts: None,
                topic: "control".to_string(),
                op: "authenticated".to_string(),
                seq: None,
                key: None,
                payload: serde_json::json!({"status":"ok"}),
            })
            .await?;
        }
        ("control", "subscribe") => {
            let req: ControlSubscribe = serde_json::from_value(frame.payload)
                .map_err(|e| format!("Invalid subscribe payload: {e}"))?;
            let key = format!("{}:{}", req.topic, uuid::Uuid::new_v4());

            {
                let mut streams = conn.streams.lock().await;
                streams.insert(key.clone(), StreamBuf::new(conn.ws_cfg.default_credits as i64));
            }

            // Start topic pump.
            let pump = match req.topic.as_str() {
                "logs" => Some(spawn_logs_pump(Arc::clone(conn), Arc::clone(state), key.clone(), req.filter)),
                "requests" => Some(spawn_requests_pump(Arc::clone(conn), Arc::clone(state), key.clone(), req.filter)),
                "nitro.events" => Some(spawn_nitro_events_pump(Arc::clone(conn), Arc::clone(state), key.clone(), req.filter)),
                "nitro.v2.events" => Some(spawn_nitro_v2_events_pump(Arc::clone(conn), Arc::clone(state), key.clone(), req.filter)),
                "nitro.txn" => Some(spawn_nitro_txn_pump(Arc::clone(conn), Arc::clone(state), key.clone(), req.filter)),
                "metrics" => Some(spawn_metrics_pump(Arc::clone(conn), Arc::clone(state), key.clone())),
                "events" => Some(spawn_events_pump(Arc::clone(conn), Arc::clone(state), key.clone(), req.filter)),
                _ => None,
            };

            if let Some(h) = pump {
                let mut streams = conn.streams.lock().await;
                if let Some(sb) = streams.get_mut(&key) {
                    sb.task = Some(h);
                }
            } else {
                let mut streams = conn.streams.lock().await;
                let _ = streams.remove(&key);
                drop(streams);
                conn.send_control_error("unsupported", "unknown topic", None).await;
                return Ok(());
            }

            // Acknowledge subscription; topic-specific tasks are wired in later phases.
            conn.enqueue_frame(WsFrame {
                v: 1,
                id: frame.id,
                ts: None,
                topic: "control".to_string(),
                op: "subscribed".to_string(),
                seq: None,
                key: Some(key.clone()),
                payload: serde_json::json!({"topic": req.topic, "key": key}),
            })
            .await?;
        }
        ("control", "unsubscribe") => {
            let req: ControlUnsubscribe = serde_json::from_value(frame.payload)
                .map_err(|e| format!("Invalid unsubscribe payload: {e}"))?;
            let mut streams = conn.streams.lock().await;
            if let Some(mut sb) = streams.remove(&req.key) {
                if let Some(h) = sb.task.take() {
                    h.abort();
                }
            }
            drop(streams);

            conn.enqueue_frame(WsFrame {
                v: 1,
                id: frame.id,
                ts: None,
                topic: "control".to_string(),
                op: "unsubscribed".to_string(),
                seq: None,
                key: Some(req.key.clone()),
                payload: serde_json::json!({"key": req.key}),
            })
            .await?;
        }
        ("control", "credits") => {
            let req: CreditsGrant = serde_json::from_value(frame.payload)
                .map_err(|e| format!("Invalid credits payload: {e}"))?;
            let grant = req.grant as i64;
            conn.grant_credits_and_flush(&req.key, grant).await;
        }
        ("control", "ack") => {
            let req: ControlAck = serde_json::from_value(frame.payload)
                .map_err(|e| format!("Invalid ack payload: {e}"))?;
            conn.ack_and_prune(&req.key, req.last_seq).await;
            if let Some(grant) = req.grant {
                conn.grant_credits_and_flush(&req.key, grant as i64).await;
            }
        }
        ("control", "resume") => {
            let req: ControlResume = serde_json::from_value(frame.payload)
                .map_err(|e| format!("Invalid resume payload: {e}"))?;
            let ttl = Duration::from_millis(conn.ws_cfg.resume_ttl_ms);

            let mut all_ok = true;
            let mut stream_status: Vec<serde_json::Value> = Vec::new();
            let mut max_seen_seq: u64 = 0;

            for ResumeStream { key, last_seq } in req.streams {
                let (replay, latest_seq, oldest_seq, source) = {
                    let streams = conn.streams.lock().await;
                    if let Some(sb) = streams.get(&key) {
                        let latest = sb.ring.back().map(|rf| rf.seq).unwrap_or(0);
                        let oldest = sb.ring.front().map(|rf| rf.seq).unwrap_or(0);
                        (sb.ring_replay_since(last_seq, ttl), latest, oldest, "conn")
                    } else if let Some((frames, latest, oldest)) =
                        global_ring_snapshot(&key, last_seq, ttl)
                    {
                        (frames, latest, oldest, "global")
                    } else {
                        (Vec::new(), 0, 0, "none")
                    }
                };

                // Up-to-date: nothing to replay, but resume is still successful.
                if latest_seq > 0 && last_seq >= latest_seq {
                    max_seen_seq = max_seen_seq.max(latest_seq);
                    stream_status.push(serde_json::json!({
                        "key": key,
                        "partial": false,
                        "reason": "up_to_date",
                        "latest_seq": latest_seq,
                        "source": source
                    }));
                    continue;
                }

                // No buffer available.
                if latest_seq == 0 {
                    if last_seq == 0 {
                        stream_status.push(serde_json::json!({
                            "key": key,
                            "partial": false,
                            "reason": "no_history",
                            "latest_seq": 0,
                            "source": source
                        }));
                    } else {
                        all_ok = false;
                        stream_status.push(serde_json::json!({
                            "key": key,
                            "partial": true,
                            "reason": "no_buffer",
                            "latest_seq": 0,
                            "source": source
                        }));
                    }
                    continue;
                }

                let mut partial = false;
                let mut reason = "ok";

                let expected_first = last_seq.saturating_add(1);
                if oldest_seq > expected_first {
                    partial = true;
                    reason = "too_old";
                }

                let actual_first = replay
                    .first()
                    .and_then(|f| f.seq)
                    .unwrap_or(0);
                let actual_last = replay
                    .last()
                    .and_then(|f| f.seq)
                    .unwrap_or(0);

                if replay.is_empty() {
                    partial = true;
                    reason = "expired_or_missing";
                } else {
                    if actual_first != expected_first {
                        partial = true;
                        reason = "gap";
                    }
                    if actual_last != latest_seq {
                        partial = true;
                        reason = "partial_tail";
                    }
                }

                if partial {
                    all_ok = false;
                }

                max_seen_seq = max_seen_seq.max(latest_seq);

                // Ensure local stream state exists post-resume (credits/backpressure).
                {
                    let mut streams = conn.streams.lock().await;
                    streams
                        .entry(key.clone())
                        .or_insert_with(|| StreamBuf::new(conn.ws_cfg.default_credits as i64));
                }

                for f in replay {
                    // Preserve original seq for resume cursors; still respects credits.
                    conn.enqueue_existing_frame(f).await?;
                }

                stream_status.push(serde_json::json!({
                    "key": key,
                    "partial": partial,
                    "reason": reason,
                    "latest_seq": latest_seq,
                    "oldest_seq": oldest_seq,
                    "requested_last_seq": last_seq,
                    "source": source
                }));
            }

            if max_seen_seq > 0 {
                let _ = conn.next_seq.fetch_max(max_seen_seq.saturating_add(1), Ordering::Relaxed);
            }

            if all_ok {
                METRICS.resume_ok.fetch_add(1, Ordering::Relaxed);
            } else {
                METRICS.resume_fail.fetch_add(1, Ordering::Relaxed);
                warn!(
                    target = "openfang_ws",
                    event = "ws.resume",
                    conn_id = %conn.conn_id,
                    partial = true,
                    streams = stream_status.len()
                );
            }
            conn.enqueue_frame(WsFrame {
                v: 1,
                id: frame.id,
                ts: None,
                topic: "control".to_string(),
                op: "resumed".to_string(),
                seq: None,
                key: None,
                payload: serde_json::json!({"partial": !all_ok, "streams": stream_status}),
            })
            .await?;
        }
        ("control", "ping") => {
            conn.enqueue_frame(WsFrame {
                v: 1,
                id: frame.id,
                ts: None,
                topic: "control".to_string(),
                op: "pong".to_string(),
                seq: None,
                key: None,
                payload: serde_json::json!({}),
            })
            .await?;
        }
        ("agent.run", "start") => {
            let req: AgentRunStart = serde_json::from_value(frame.payload)
                .map_err(|e| format!("Invalid agent.run.start payload: {e}"))?;
            let max_msg = conn.ws_cfg.max_frame_bytes.min(2 * 1024 * 1024);
            if req.message.len() > max_msg {
                conn.send_control_error("bad_request", "message too large", None)
                    .await;
                return Ok(());
            }

            let agent_id: AgentId = req
                .agent_id
                .parse()
                .map_err(|_| "Invalid agent_id".to_string())?;

            if state.kernel.registry.get(agent_id).is_none() {
                conn.send_control_error("bad_request", "unknown agent_id", None)
                    .await;
                return Ok(());
            }

            let key = format!("agent:{}:run:{}", agent_id, uuid::Uuid::new_v4());

            // Insert stream state (so resume works even before the first frame is sent).
            {
                let mut streams = conn.streams.lock().await;
                streams.insert(
                    key.clone(),
                    StreamBuf::new(conn.ws_cfg.default_credits as i64),
                );
            }

            // Start ack.
            conn.enqueue_frame(WsFrame {
                v: 1,
                id: frame.id.clone(),
                ts: None,
                topic: "agent.run".to_string(),
                op: "start".to_string(),
                seq: None,
                key: Some(key.clone()),
                payload: serde_json::json!({
                    "agent_id": agent_id.to_string(),
                    "stream": true,
                }),
            })
            .await?;

            let kernel_handle: Arc<dyn KernelHandle> =
                state.kernel.clone() as Arc<dyn KernelHandle>;
            let (mut rx, run_handle) = state
                .kernel
                .send_message_streaming(agent_id, &req.message, Some(kernel_handle), None)
                .map_err(|e| e.to_string())?;

            let run_conn = Arc::clone(conn);
            let run_state = Arc::clone(state);
            let key_clone = key.clone();
            let (agent_name, model_provider, model_name) = state
                .kernel
                .registry
                .get(agent_id)
                .map(|e| {
                    (
                        e.name.clone(),
                        e.manifest.model.provider.clone(),
                        e.manifest.model.model.clone(),
                    )
                })
                .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string(), "unknown".to_string()));
            let task = tokio::spawn(async move {
                let started_at = Instant::now();
                // Stream events.
                while let Some(event) = rx.recv().await {
                    let frame_opt = match event {
                        StreamEvent::TextDelta { text } => Some(WsFrame {
                            v: 1,
                            id: None,
                            ts: None,
                            topic: "agent.run".to_string(),
                            op: "delta".to_string(),
                            seq: None,
                            key: Some(key_clone.clone()),
                            payload: serde_json::json!({"text": text}),
                        }),
                        StreamEvent::ToolUseStart { id, name } => Some(WsFrame {
                            v: 1,
                            id: None,
                            ts: None,
                            topic: "agent.run".to_string(),
                            op: "tool".to_string(),
                            seq: None,
                            key: Some(key_clone.clone()),
                            payload: serde_json::json!({"id": id, "name": name}),
                        }),
                        StreamEvent::ToolInputDelta { text } => Some(WsFrame {
                            v: 1,
                            id: None,
                            ts: None,
                            topic: "agent.run".to_string(),
                            op: "tool_input".to_string(),
                            seq: None,
                            key: Some(key_clone.clone()),
                            payload: serde_json::json!({"text": text}),
                        }),
                        StreamEvent::ToolUseEnd { id, name, input } => Some(WsFrame {
                            v: 1,
                            id: None,
                            ts: None,
                            topic: "agent.run".to_string(),
                            op: "tool".to_string(),
                            seq: None,
                            key: Some(key_clone.clone()),
                            payload: serde_json::json!({"id": id, "name": name, "input": input}),
                        }),
                        StreamEvent::ToolExecutionResult {
                            tool_use_id,
                            name,
                            result_preview,
                            is_error,
                        } => Some(WsFrame {
                            v: 1,
                            id: None,
                            ts: None,
                            topic: "agent.run".to_string(),
                            op: "result".to_string(),
                            seq: None,
                            key: Some(key_clone.clone()),
                            payload: serde_json::json!({
                                "tool_use_id": tool_use_id,
                                "name": name,
                                "content": result_preview,
                                "is_error": is_error,
                            }),
                        }),
                        StreamEvent::PhaseChange { phase, detail } => Some(WsFrame {
                            v: 1,
                            id: None,
                            ts: None,
                            topic: "agent.run".to_string(),
                            op: "phase".to_string(),
                            seq: None,
                            key: Some(key_clone.clone()),
                            payload: serde_json::json!({"phase": phase, "detail": detail}),
                        }),
                        // `ContentComplete` is followed by the join handle result;
                        // WS `done` is emitted after we have full accounting (cost, iterations, tool_calls).
                        StreamEvent::ContentComplete { .. } => None,
                        StreamEvent::ThinkingDelta { .. } => None,
                    };

                    if let Some(f) = frame_opt {
                        let _ = run_conn.enqueue_frame(f).await;
                    }
                }

                // Final `done`.
                let (done_payload, outcome, tokens_in, tokens_out, iterations, tool_calls, cost_usd, silent, err) =
                    match run_handle.await {
                        Ok(Ok(result)) => (
                            serde_json::json!({
                                "response": result.response,
                                "tokens": {
                                    "in": result.total_usage.input_tokens,
                                    "out": result.total_usage.output_tokens,
                                },
                                "iterations": result.iterations,
                                "tool_calls": result.tool_calls,
                                "cost_usd": result.cost_usd,
                                "silent": result.silent,
                            }),
                            "success",
                            Some(result.total_usage.input_tokens),
                            Some(result.total_usage.output_tokens),
                            Some(result.iterations),
                            Some(result.tool_calls),
                            result.cost_usd.unwrap_or(0.0),
                            Some(result.silent),
                            None,
                        ),
                        Ok(Err(e)) => {
                        let _ = run_state.kernel.audit_log.record(
                            agent_id.to_string(),
                            openfang_runtime::audit::AuditAction::AgentMessage,
                            "ws agent.run failed",
                            format!("error: {e}"),
                        );
                        (
                            serde_json::json!({"error": e.to_string()}),
                            "error",
                            None,
                            None,
                            None,
                            None,
                            0.0,
                            None,
                            Some(e.to_string()),
                        )
                    }
                    Err(e) => (
                        serde_json::json!({"error": format!("join error: {e}")}),
                        "error",
                        None,
                        None,
                        None,
                        None,
                        0.0,
                        None,
                        Some(format!("join error: {e}")),
                    ),
                };

                let _ = run_conn
                    .enqueue_frame(WsFrame {
                        v: 1,
                        id: None,
                        ts: None,
                        topic: "agent.run".to_string(),
                        op: "done".to_string(),
                        seq: None,
                        key: Some(key_clone.clone()),
                        payload: done_payload,
                    })
                    .await;

                // Wide event: one structured run summary on completion.
                info!(
                    target = "openfang_ws",
                    event = "ws.agent_run",
                    agent_id = %agent_id,
                    agent_name,
                    model_provider,
                    model_name,
                    stream_key = %key_clone,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    outcome,
                    tokens_in = tokens_in.unwrap_or(0),
                    tokens_out = tokens_out.unwrap_or(0),
                    iterations = iterations.unwrap_or(0),
                    tool_calls = tool_calls.unwrap_or(0),
                    cost_usd,
                    silent = silent.unwrap_or(false),
                    error = err.as_deref().unwrap_or(""),
                );
            });

            // Track task handle so unsubscribe can abort it.
            {
                let mut streams = conn.streams.lock().await;
                if let Some(sb) = streams.get_mut(&key) {
                    sb.task = Some(task);
                }
            }
        }
        ("tool.exec", "start") => {
            let req: ToolExecStart = serde_json::from_value(frame.payload)
                .map_err(|e| format!("Invalid tool.exec.start payload: {e}"))?;

            let key = format!("tool.exec:{}", uuid::Uuid::new_v4());
            let tool_use_id = frame
                .id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let tool_name = req.tool;
            let args = req.arguments;
            let caller_agent_id = req.agent_id;
            let stream = req.stream;

            // Insert stream state (so resume works even before the first frame is sent).
            {
                let mut streams = conn.streams.lock().await;
                streams.insert(key.clone(), StreamBuf::new(conn.ws_cfg.default_credits as i64));
            }

            // Start ack.
            conn.enqueue_frame(WsFrame {
                v: 1,
                id: frame.id.clone(),
                ts: None,
                topic: "tool.exec".to_string(),
                op: "start".to_string(),
                seq: None,
                key: Some(key.clone()),
                payload: serde_json::json!({
                    "tool_use_id": tool_use_id.clone(),
                    "tool": tool_name.clone(),
                    "stream": stream
                }),
            })
            .await?;

            // Snapshot skill registry before async call (RwLockReadGuard is !Send)
            let skill_snapshot = state
                .kernel
                .skill_registry
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .snapshot();

            // Discover tool catalog for validation (builtin + skills + MCP).
            let mut tools = openfang_runtime::tool_runner::builtin_tool_definitions()
                .iter()
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>();
            for t in skill_snapshot.all_tool_definitions() {
                tools.push(t.name);
            }
            if let Ok(mcp_tools) = state.kernel.mcp_tools.lock() {
                for t in mcp_tools.iter() {
                    tools.push(t.name.clone());
                }
            }

            if !tools.iter().any(|t| t == &tool_name) {
                conn.enqueue_frame(WsFrame {
                    v: 1,
                    id: None,
                    ts: None,
                    topic: "tool.exec".to_string(),
                    op: "done".to_string(),
                    seq: None,
                    key: Some(key.clone()),
                    payload: serde_json::json!({
                        "tool_use_id": tool_use_id.clone(),
                        "tool": tool_name,
                        "is_error": true,
                        "error": "Unknown tool"
                    }),
                })
                .await?;
                return Ok(());
            }

            let run_conn = Arc::clone(conn);
            let run_state = Arc::clone(state);
            let key_clone = key.clone();
            let caller_agent_id_clone = caller_agent_id.clone();

            let task = tokio::spawn(async move {
                let kh = run_state.kernel.clone() as Arc<dyn KernelHandle>;

                let started_at = Instant::now();
                let result = openfang_runtime::tool_runner::execute_tool(
                    &tool_use_id,
                    &tool_name,
                    &args,
                    Some(&kh),
                    None,
                    caller_agent_id_clone.as_deref(),
                    Some(&skill_snapshot),
                    Some(&run_state.kernel.mcp_connections),
                    Some(&run_state.kernel.web_ctx),
                    Some(&run_state.kernel.browser_ctx),
                    Some(&run_state.kernel.desktop_ctx),
                    None,
                    None,
                    Some(&run_state.kernel.media_engine),
                    None,
                    if run_state.kernel.config.tts.enabled {
                        Some(&run_state.kernel.tts_engine)
                    } else {
                        None
                    },
                    if run_state.kernel.config.docker.enabled {
                        Some(&run_state.kernel.config.docker)
                    } else {
                        None
                    },
                    Some(&*run_state.kernel.process_manager),
                    None,
                    None,
                    None,
                )
                .await;

                let content = result.content;
                if stream && !content.is_empty() {
                    for chunk in content.as_bytes().chunks(4096) {
                        let text = String::from_utf8_lossy(chunk).to_string();
                        let _ = run_conn
                            .enqueue_frame(WsFrame {
                                v: 1,
                                id: None,
                                ts: None,
                                topic: "tool.exec".to_string(),
                                op: "delta".to_string(),
                                seq: None,
                                key: Some(key_clone.clone()),
                                payload: serde_json::json!({"text": text}),
                            })
                            .await;
                    }
                }

                let _ = run_conn
                    .enqueue_frame(WsFrame {
                        v: 1,
                        id: None,
                        ts: None,
                        topic: "tool.exec".to_string(),
                        op: "done".to_string(),
                        seq: None,
                        key: Some(key_clone.clone()),
                        payload: serde_json::json!({
                            "tool_use_id": tool_use_id,
                            "tool": tool_name.clone(),
                            "content": content,
                            "is_error": result.is_error
                        }),
                    })
                    .await;

                // Also record to audit log for visibility in logs topic.
                let agent_id = caller_agent_id.unwrap_or_else(|| "ws".to_string());
                let _ = run_state.kernel.audit_log.record(
                    agent_id,
                    openfang_runtime::audit::AuditAction::ToolInvoke,
                    "ws tool.exec",
                    format!("tool={tool_name} is_error={}", result.is_error),
                );

                // Wide event: one structured tool execution summary.
                info!(
                    target = "openfang_ws",
                    event = "ws.tool_exec",
                    tool_use_id = %tool_use_id,
                    tool = tool_name,
                    stream_key = %key_clone,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    is_error = result.is_error,
                    content_bytes = content.len(),
                );
            });

            // Track task handle so unsubscribe can abort it.
            {
                let mut streams = conn.streams.lock().await;
                if let Some(sb) = streams.get_mut(&key) {
                    sb.task = Some(task);
                }
            }
        }
        _ => {
            conn.send_control_error("bad_request", "unknown topic/op", None)
                .await;
        }
    }
    Ok(())
}

fn auth_token_ok(api_key: &str, token: &str) -> bool {
    if api_key.is_empty() {
        return true;
    }
    use subtle::ConstantTimeEq;
    token.len() == api_key.len() && bool::from(token.as_bytes().ct_eq(api_key.as_bytes()))
}

fn classify_audit_level(action: &str) -> &'static str {
    let a = action.to_lowercase();
    if a.contains("error") || a.contains("fail") || a.contains("crash") || a.contains("denied") {
        "error"
    } else if a.contains("warn") || a.contains("block") || a.contains("kill") {
        "warn"
    } else {
        "info"
    }
}

fn spawn_logs_pump(
    conn: Arc<Conn>,
    state: Arc<AppState>,
    key: String,
    filter: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let level_filter = filter
            .get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let agent_filter = filter
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let text_filter = filter
            .get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let mut last_seq: u64 = 0;
        loop {
            if conn.closing.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;

            let entries = state.kernel.audit_log.recent(200);
            for entry in &entries {
                if entry.seq <= last_seq {
                    continue;
                }

                if !level_filter.is_empty() {
                    let classified = classify_audit_level(&format!("{:?}", entry.action));
                    if classified != level_filter {
                        continue;
                    }
                }

                if !agent_filter.is_empty() && entry.agent_id != agent_filter {
                    continue;
                }

                if !text_filter.is_empty() {
                    let haystack = format!(
                        "{:?} {} {}",
                        entry.action, entry.detail, entry.agent_id
                    )
                    .to_lowercase();
                    if !haystack.contains(&text_filter) {
                        continue;
                    }
                }

                let payload = serde_json::json!({
                    "seq": entry.seq,
                    "timestamp": entry.timestamp,
                    "agent_id": entry.agent_id,
                    "action": format!("{:?}", entry.action),
                    "detail": entry.detail,
                    "outcome": entry.outcome,
                    "hash": entry.hash,
                });

                let _ = conn
                    .enqueue_frame(WsFrame {
                        v: 1,
                        id: None,
                        ts: None,
                        topic: "logs".to_string(),
                        op: "event".to_string(),
                        seq: None,
                        key: Some(key.clone()),
                        payload,
                    })
                    .await;
            }
            if let Some(last) = entries.last() {
                last_seq = last.seq;
            }
        }
    })
}

fn spawn_requests_pump(
    conn: Arc<Conn>,
    state: Arc<AppState>,
    key: String,
    filter: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let text_filter = filter
            .get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let status_filter: Option<u16> = filter
            .get("status")
            .and_then(|v| v.as_u64())
            .map(|v| v as u16);

        let mut last_seq: u64 = 0;
        loop {
            if conn.closing.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;

            let entries = state.wide_event_log.recent(200);
            for entry in &entries {
                if entry.seq <= last_seq {
                    continue;
                }

                if let Some(sf) = status_filter {
                    if entry.status != sf {
                        continue;
                    }
                }

                if !text_filter.is_empty() {
                    let haystack = format!("{} {} {}", entry.method, entry.path, entry.request_id)
                        .to_lowercase();
                    if !haystack.contains(&text_filter) {
                        continue;
                    }
                }

                let payload = serde_json::to_value(entry).unwrap_or_default();

                let _ = conn
                    .enqueue_frame(WsFrame {
                        v: 1,
                        id: None,
                        ts: None,
                        topic: "requests".to_string(),
                        op: "event".to_string(),
                        seq: None,
                        key: Some(key.clone()),
                        payload,
                    })
                    .await;
            }
            if let Some(last) = entries.last() {
                last_seq = last.seq;
            }
        }
    })
}

fn spawn_nitro_events_pump(
    conn: Arc<Conn>,
    state: Arc<AppState>,
    key: String,
    filter: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut after_seq: i64 = filter
            .get("after_seq")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let agent_filter = filter
            .get("agent_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let home_dir = state.kernel.config.home_dir.clone();
        let workspaces_dir = state.kernel.config.effective_workspaces_dir();
        let memory = Arc::clone(&state.kernel.memory);

        loop {
            if conn.closing.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;

            let events = tokio::task::spawn_blocking({
                let home_dir = home_dir.clone();
                let workspaces_dir = workspaces_dir.clone();
                let memory = Arc::clone(&memory);
                move || {
                    let mgr = NitroComputerManager::new(memory.as_ref(), home_dir, workspaces_dir, false);
                    mgr.list_events_since(after_seq, 200)
                }
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();

            for ev in &events {
                if let Some(agent_id) = agent_filter.as_deref() {
                    let payload_agent = ev
                        .get("payload")
                        .and_then(|p| p.get("agent_id"))
                        .and_then(|v| v.as_str());
                    if payload_agent != Some(agent_id) {
                        continue;
                    }
                }

                let seq = ev.get("seq").and_then(|v| v.as_i64()).unwrap_or(after_seq);
                after_seq = after_seq.max(seq);

                let _ = conn
                    .enqueue_frame(WsFrame {
                        v: 1,
                        id: None,
                        ts: None,
                        topic: "nitro.events".to_string(),
                        op: "event".to_string(),
                        seq: None,
                        key: Some(key.clone()),
                        payload: ev.clone(),
                    })
                    .await;
            }
        }
    })
}

fn spawn_nitro_v2_events_pump(
    conn: Arc<Conn>,
    state: Arc<AppState>,
    key: String,
    filter: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut after_id: i64 = filter
            .get("after_id")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let agent_filter = filter
            .get("agent_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let home_dir = state.kernel.config.home_dir.clone();
        let workspaces_dir = state.kernel.config.effective_workspaces_dir();
        let memory = Arc::clone(&state.kernel.memory);

        loop {
            if conn.closing.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;

            let events = tokio::task::spawn_blocking({
                let home_dir = home_dir.clone();
                let workspaces_dir = workspaces_dir.clone();
                let memory = Arc::clone(&memory);
                move || {
                    let mgr = NitroComputerManager::new(memory.as_ref(), home_dir, workspaces_dir, false);
                    mgr.list_v2_events_since(after_id, 200)
                }
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();

            for ev in &events {
                if let Some(agent_id) = agent_filter.as_deref() {
                    let payload_agent = ev
                        .get("payload")
                        .and_then(|p| p.get("agent_id"))
                        .and_then(|v| v.as_str());
                    if payload_agent != Some(agent_id) {
                        continue;
                    }
                }

                let id = ev.get("id").and_then(|v| v.as_i64()).unwrap_or(after_id);
                after_id = after_id.max(id);

                let _ = conn
                    .enqueue_frame(WsFrame {
                        v: 1,
                        id: None,
                        ts: None,
                        topic: "nitro.v2.events".to_string(),
                        op: "event".to_string(),
                        seq: None,
                        key: Some(key.clone()),
                        payload: ev.clone(),
                    })
                    .await;
            }
        }
    })
}

fn spawn_nitro_txn_pump(
    conn: Arc<Conn>,
    state: Arc<AppState>,
    key: String,
    filter: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let limit: usize = filter.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
        let home_dir = state.kernel.config.home_dir.clone();
        let workspaces_dir = state.kernel.config.effective_workspaces_dir();
        let memory = Arc::clone(&state.kernel.memory);

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        loop {
            if conn.closing.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;

            let txns = tokio::task::spawn_blocking({
                let home_dir = home_dir.clone();
                let workspaces_dir = workspaces_dir.clone();
                let memory = Arc::clone(&memory);
                move || {
                    let mgr = NitroComputerManager::new(memory.as_ref(), home_dir, workspaces_dir, false);
                    mgr.list_txns(limit)
                }
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();

            // Oldest-to-newest ordering for stable stream.
            for txn in txns.into_iter().rev() {
                if !seen.insert(txn.txn_id.clone()) {
                    continue;
                }
                let payload = serde_json::to_value(&txn).unwrap_or_else(|_| {
                    serde_json::json!({"txn_id": txn.txn_id, "error": "serialize_failed"})
                });
                let _ = conn
                    .enqueue_frame(WsFrame {
                        v: 1,
                        id: None,
                        ts: None,
                        topic: "nitro.txn".to_string(),
                        op: "event".to_string(),
                        seq: None,
                        key: Some(key.clone()),
                        payload,
                    })
                    .await;
            }
        }
    })
}

fn spawn_metrics_pump(
    conn: Arc<Conn>,
    state: Arc<AppState>,
    key: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            if conn.closing.load(Ordering::Relaxed) {
                break;
            }
            let agents = state.kernel.registry.list();
            let active = agents
                .iter()
                .filter(|a| matches!(a.state, openfang_types::agent::AgentState::Running))
                .count();
            let payload = serde_json::json!({
                "agents_active": active,
                "agents_total": agents.len(),
                "ws": metrics_snapshot(),
            });
            let _ = conn
                .enqueue_frame(WsFrame {
                    v: 1,
                    id: None,
                    ts: None,
                    topic: "metrics".to_string(),
                    op: "gauge".to_string(),
                    seq: None,
                    key: Some(key.clone()),
                    payload,
                })
                .await;
        }
    })
}

fn spawn_events_pump(
    conn: Arc<Conn>,
    state: Arc<AppState>,
    key: String,
    filter: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let agent_filter = filter
            .get("agent_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let kind_filter = filter
            .get("kind")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let limit: usize = filter
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| (v as usize).min(500))
            .unwrap_or(50);

        // Replay recent history.
        let history = state.kernel.event_bus.history(limit).await;
        // history() returns newest-first; reverse to send oldest-first.
        for event in history.into_iter().rev() {
            if conn.closing.load(Ordering::Relaxed) {
                return;
            }
            if !event_matches_filter(&event, &agent_filter, &kind_filter) {
                continue;
            }
            let payload = event_to_json(&event);
            let _ = conn
                .enqueue_frame(WsFrame {
                    v: 1,
                    id: None,
                    ts: None,
                    topic: "events".to_string(),
                    op: "event".to_string(),
                    seq: None,
                    key: Some(key.clone()),
                    payload,
                })
                .await;
        }

        // Live stream via broadcast receiver.
        let mut rx = state.kernel.event_bus.subscribe_all();
        loop {
            if conn.closing.load(Ordering::Relaxed) {
                break;
            }
            match rx.recv().await {
                Ok(event) => {
                    if !event_matches_filter(&event, &agent_filter, &kind_filter) {
                        continue;
                    }
                    let payload = event_to_json(&event);
                    let _ = conn
                        .enqueue_frame(WsFrame {
                            v: 1,
                            id: None,
                            ts: None,
                            topic: "events".to_string(),
                            op: "event".to_string(),
                            seq: None,
                            key: Some(key.clone()),
                            payload,
                        })
                        .await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("events pump lagged by {n} events");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    })
}

/// Check if an event matches the optional agent_id and kind filters.
fn event_matches_filter(
    event: &openfang_types::event::Event,
    agent_filter: &Option<String>,
    kind_filter: &Option<String>,
) -> bool {
    use openfang_types::event::EventTarget;

    if let Some(ref agent_id) = agent_filter {
        let matches_target = match &event.target {
            EventTarget::Agent(id) => id.to_string() == *agent_id,
            _ => false,
        };
        let matches_source = event.source.to_string() == *agent_id;
        if !matches_target && !matches_source {
            return false;
        }
    }

    if let Some(ref kind) = kind_filter {
        let payload_kind = event_payload_kind(&event.payload);
        if payload_kind != *kind {
            return false;
        }
    }

    true
}

/// Extract a short kind string from an EventPayload variant.
fn event_payload_kind(payload: &openfang_types::event::EventPayload) -> &'static str {
    use openfang_types::event::EventPayload;
    match payload {
        EventPayload::Message(_) => "message",
        EventPayload::ToolResult(_) => "tool_result",
        EventPayload::MemoryUpdate(_) => "memory_update",
        EventPayload::Lifecycle(_) => "lifecycle",
        EventPayload::Network(_) => "network",
        EventPayload::System(_) => "system",
        EventPayload::Custom(_) => "custom",
    }
}

/// Convert an Event to a JSON value for the WS frame payload.
fn event_to_json(event: &openfang_types::event::Event) -> serde_json::Value {
    serde_json::json!({
        "id": event.id.to_string(),
        "source": event.source.to_string(),
        "target": serde_json::to_value(&event.target).unwrap_or_default(),
        "kind": event_payload_kind(&event.payload),
        "payload": serde_json::to_value(&event.payload).unwrap_or_default(),
        "ts": event.timestamp.to_rfc3339(),
        "correlation_id": event.correlation_id.map(|c| c.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_token_ok_empty_key_allows() {
        assert!(auth_token_ok("", "anything"));
    }

    #[test]
    fn test_auth_token_ok_mismatch_rejects() {
        assert!(!auth_token_ok("secret", "wrong"));
        assert!(auth_token_ok("secret", "secret"));
    }

    #[test]
    fn test_streambuf_replay_filters_seq() {
        let mut sb = StreamBuf::new(32);
        let ttl = Duration::from_secs(3600);

        let f1 = WsFrame {
            v: 1,
            id: None,
            ts: Some("t".to_string()),
            topic: "t".to_string(),
            op: "o".to_string(),
            seq: Some(1),
            key: Some("k".to_string()),
            payload: serde_json::json!({"n": 1}),
        };
        let f2 = WsFrame {
            seq: Some(2),
            payload: serde_json::json!({"n": 2}),
            ..f1.clone()
        };
        let f3 = WsFrame {
            seq: Some(3),
            payload: serde_json::json!({"n": 3}),
            ..f1.clone()
        };

        sb.ring_push(1, f1, Instant::now());
        sb.ring_push(2, f2, Instant::now());
        sb.ring_push(3, f3, Instant::now());

        let replay = sb.ring_replay_since(1, ttl);
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0].seq, Some(2));
        assert_eq!(replay[1].seq, Some(3));
    }
}
