//! Cloud access (see CLOUD.md): a WebSocket out to the relay, one end-to-end encrypted session per
//! phone, and every phone request run through the same router and guard as on the Wi-Fi.
//!
//! - identity: the room, its token and the PC's static key (`<root>/phone/cloud.json`).
//! - `start` / `stop`: the relay connection, reconnecting with backoff (`state::cloud_status()`).
//! - `serve`: the relay's messages to sessions; tests drive it without a socket.
//! - `session`: the handshake, asking on the Mobile page before a new phone is added, then
//!   tunnelled requests with per-stream flow control.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::{Body, HttpBody as _};
use axum::http::{header, HeaderValue, Method, Request, Uri};
use axum::Router;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use http_body_util::BodyExt as _;
use p256::ecdh::EphemeralSecret;
use p256::SecretKey;
use rand::rngs::OsRng;
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch, Semaphore};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tower::ServiceExt as _;

use crate::auth::{self, CloudPeer, Device};
use crate::cloud_crypto::{self as cc, Opener, Sealer, Transcript};
use crate::state;

/// The relay; `MANGAVOX_CLOUD_URL` points a dev build at a local one (`wrangler dev`).
const DEFAULT_RELAY: &str = "https://app.bhippi.com/mangavox";

/// Time for each handshake step.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(20);
/// How long a new phone waits for "Allow" on the Mobile page.
pub const APPROVAL_WAIT: Duration = Duration::from_secs(120);
/// Largest DATA chunk, and bytes a stream may have sent but not acked.
const CHUNK: usize = 128 * 1024;
const WINDOW: usize = 1 << 20;
/// A response this small (known size) goes out with the control messages, ahead of bulk data.
const SMALL: u64 = 64 * 1024;
/// Relay messages waiting for the socket. Kept tiny so a control message or a small response
/// never waits behind more than a couple of chunks (CLOUD.md, "On the PC").
const OUTBOX: usize = 2;
const MAX_STREAMS: usize = 32;
const MAX_PHONES: usize = 8;
/// Largest frame a phone may send (a request with a 256 KiB body, base64).
const MAX_FRAME: usize = 512 * 1024;
const MAX_BODY: usize = 256 * 1024;
const INBOX: usize = 256;
const PING_EVERY: Duration = Duration::from_secs(30);
/// No message from the relay for this long: the connection is dead.
const DEAD_AFTER: Duration = Duration::from_secs(75);

/// `x-mangavox` is the app's CSRF header: a form or link from another site cannot send it.
const REQ_HEADERS: &[&str] = &["range", "if-range", "content-type", "accept", "x-mangavox"];
const RESP_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "content-range",
    "accept-ranges",
    "cache-control",
    "etag",
    "last-modified",
    "content-disposition",
    "content-security-policy",
    "x-frame-options",
    "x-content-type-options",
    "referrer-policy",
];

// ---------------------------------------------------------------- identity

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    room: String,
    token: String,
    key: String,
}

#[derive(Clone)]
pub(crate) struct Identity {
    room: String,
    room_bytes: Vec<u8>,
    token: String,
    key: SecretKey,
}

impl Identity {
    fn generate() -> Self {
        let mut room = [0u8; 16];
        OsRng.fill_bytes(&mut room);
        Self { room: cc::b64(&room), room_bytes: room.to_vec(), token: auth::random_b64(32), key: SecretKey::random(&mut OsRng) }
    }

    fn from_stored(s: &StoredIdentity) -> Option<Self> {
        let room_bytes = URL_SAFE_NO_PAD.decode(&s.room).ok().filter(|b| b.len() == 16)?;
        URL_SAFE_NO_PAD.decode(&s.token).ok().filter(|b| b.len() == 32)?;
        let key = SecretKey::from_slice(&URL_SAFE_NO_PAD.decode(&s.key).ok()?).ok()?;
        Some(Self { room: s.room.clone(), room_bytes, token: s.token.clone(), key })
    }

    fn stored(&self) -> StoredIdentity {
        StoredIdentity { room: self.room.clone(), token: self.token.clone(), key: cc::b64(&self.key.to_bytes()) }
    }
}

static IDENTITY: Mutex<Option<Identity>> = Mutex::new(None);

fn identity_path() -> PathBuf {
    mv_core::paths::root().join("phone").join("cloud.json")
}

/// This PC's cloud identity, created on first use. `None` when the file exists but cannot be
/// read (it is never replaced then: that would sign every cloud phone out).
pub(crate) fn identity() -> Option<Identity> {
    let mut g = IDENTITY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = g.as_ref() {
        return Some(i.clone());
    }
    let path = identity_path();
    let id = match std::fs::read(&path) {
        Ok(raw) => Identity::from_stored(&serde_json::from_slice(&raw).ok()?)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = Identity::generate();
            let write = || -> std::io::Result<()> {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let tmp = path.with_extension(format!("json.{}.part", std::process::id()));
                std::fs::write(&tmp, serde_json::to_vec_pretty(&id.stored()).unwrap_or_default())?;
                std::fs::rename(&tmp, &path)
            };
            if let Err(e) = write() {
                eprintln!("[cloud] could not save the cloud key: {e}");
                return None;
            }
            id
        }
        Err(_) => return None,
    };
    *g = Some(id.clone());
    Some(id)
}

#[cfg(test)]
pub(crate) fn use_identity(id: Identity) {
    *IDENTITY.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
}

/// The relay's address, without a trailing slash.
pub fn base_url() -> String {
    std::env::var("MANGAVOX_CLOUD_URL").ok().map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty()).unwrap_or_else(|| DEFAULT_RELAY.into())
}

/// The PC's WebSocket address: TLS, except for a relay on this machine.
fn ws_url(base: &str, room: &str) -> Result<String, String> {
    let rest = if let Some(r) = base.strip_prefix("https://") {
        format!("wss://{r}")
    } else if let Some(r) = base.strip_prefix("http://").filter(|r| r.starts_with("localhost") || r.starts_with("127.0.0.1")) {
        format!("ws://{r}")
    } else {
        return Err(format!("The relay address must use https: {base}"));
    };
    Ok(format!("{rest}/ws/pc?room={room}"))
}

/// What the QR code holds with cloud access on (CLOUD.md, "QR code").
pub fn pair_link(code: &str, lan: Option<&str>) -> Option<String> {
    let id = identity()?;
    let fp = cc::fingerprint(&id.key.public_key());
    let mut url = format!("{}/#p={}.{code}.{fp}", base_url(), id.room);
    if let Some(l) = lan {
        url.push_str("&l=");
        url.push_str(&uri_encode(l));
    }
    Some(url)
}

fn uri_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn pc_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "This PC".into())
}

// ---------------------------------------------------------------- approvals

/// A phone waiting for "Allow" on the Mobile page.
#[derive(Clone, Debug)]
pub struct Pending {
    pub id: u64,
    pub name: String,
    /// The code the phone shows (CLOUD.md, "Handshake"): allow only when they match.
    pub code: String,
    pub asked: Instant,
}

static PENDING: Mutex<Vec<(Pending, oneshot::Sender<bool>)>> = Mutex::new(Vec::new());
static NEXT_ASK: AtomicU64 = AtomicU64::new(1);

/// Phones waiting for an answer, oldest first.
pub fn pending() -> Vec<Pending> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(p, _)| p.clone()).collect()
}

/// Allows or denies a waiting phone.
pub fn answer(id: u64, allow: bool) {
    let found = {
        let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().position(|(p, _)| p.id == id).map(|i| g.remove(i))
    };
    if let Some((_, tx)) = found {
        let _ = tx.send(allow);
    }
    state::changed();
}

fn ask(name: String, code: String) -> (u64, oneshot::Receiver<bool>) {
    let id = NEXT_ASK.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push((Pending { id, name, code, asked: Instant::now() }, tx));
    state::changed();
    (id, rx)
}

/// Takes a question off the Mobile page when its session ends without an answer.
struct AskGuard(u64);

impl Drop for AskGuard {
    fn drop(&mut self) {
        let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let before = g.len();
        g.retain(|(p, _)| p.id != self.0);
        if g.len() != before {
            state::changed();
        }
    }
}

// ---------------------------------------------------------------- start / stop

struct Running {
    generation: u64,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

static RUN: Mutex<Option<Running>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn is_current(generation: u64) -> bool {
    RUN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|r| r.generation == generation)
}

/// Connects to the relay and keeps the connection up (no-op when running).
pub fn start() {
    let mut g = RUN.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_some() {
        return;
    }
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let (tx, rx) = watch::channel(false);
    let task = mv_ui::bg::runtime().spawn(run(generation, rx));
    *g = Some(Running { generation, stop: tx, task });
    drop(g);
    state::set_cloud_status(state::CloudStatus { running: true, connected: false, error: None });
}

/// Disconnects from the relay; every cloud session ends and waiting phones are denied.
pub fn stop() {
    let r = RUN.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(r) = r {
        let _ = r.stop.send(true);
        let task = r.task;
        mv_ui::bg::runtime().spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            task.abort();
        });
        eprintln!("[cloud] stopped");
    }
    for p in pending() {
        answer(p.id, false);
    }
    state::set_cloud_status(state::CloudStatus::default());
}

async fn run(generation: u64, mut stop: watch::Receiver<bool>) {
    let mut backoff = 1u64;
    loop {
        if *stop.borrow() || !is_current(generation) {
            return;
        }
        let started = Instant::now();
        let res = connect_and_serve(&mut stop, generation).await;
        if *stop.borrow() || !is_current(generation) {
            return;
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = 1;
        }
        let error = res.err();
        if let Some(e) = &error {
            eprintln!("[cloud] {e}; retrying in {backoff} s");
        }
        state::set_cloud_status(state::CloudStatus { running: true, connected: false, error });
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
            _ = stop.changed() => return,
        }
        backoff = (backoff * 2).min(60);
    }
}

fn tls() -> Option<Arc<rustls::ClientConfig>> {
    static CFG: OnceLock<Option<Arc<rustls::ClientConfig>>> = OnceLock::new();
    CFG.get_or_init(|| {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().ok()?.with_root_certificates(roots).with_no_client_auth();
        Some(Arc::new(cfg))
    })
    .clone()
}

fn describe(e: tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match e {
        Error::Http(r) if matches!(r.status().as_u16(), 401 | 403) => "The relay refused this PC's key (another MangaVox may use this room)".into(),
        Error::Http(r) => format!("The relay answered {}", r.status()),
        e => format!("Could not reach the relay: {e}"),
    }
}

async fn connect_and_serve(stop: &mut watch::Receiver<bool>, generation: u64) -> Result<(), String> {
    let id = identity().ok_or("Could not read or create the cloud key file (phone/cloud.json in the data folder)")?;
    let url = ws_url(&base_url(), &id.room)?;
    let mut req = url.as_str().into_client_request().map_err(|e| e.to_string())?;
    let auth = HeaderValue::from_str(&format!("Bearer {}", id.token)).map_err(|e| e.to_string())?;
    req.headers_mut().insert(header::AUTHORIZATION, auth);
    let connector = match url.starts_with("wss:") {
        true => Some(tokio_tungstenite::Connector::Rustls(tls().ok_or("TLS is unavailable")?)),
        false => None,
    };
    let cfg = WebSocketConfig::default().max_message_size(Some(2 << 20)).max_frame_size(Some(2 << 20));
    // Nagle off: with it, the last partial packet of every frame waits for the relay's delayed
    // ACK (up to 200 ms), which capped a tunnelled download at a few hundred KB/s.
    let connect = tokio_tungstenite::connect_async_tls_with_config(req, Some(cfg), true, connector);
    let (ws, _) = tokio::time::timeout(Duration::from_secs(20), connect).await.map_err(|_| "Timed out reaching the relay".to_string())?.map_err(describe)?;
    if !is_current(generation) {
        return Ok(());
    }
    eprintln!("[cloud] connected to {}", base_url());
    state::set_cloud_status(state::CloudStatus { running: true, connected: true, error: None });

    let (mut sink, mut stream) = ws.split();
    let (in_tx, in_rx) = mpsc::channel(1024);
    let (out_tx, mut out_rx) = mpsc::channel(OUTBOX);
    let serving = tokio::spawn(serve(in_rx, out_tx, crate::server::router()));
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut heard = Instant::now();
    let res = loop {
        tokio::select! {
            _ = stop.changed() => break Ok(()),
            Some(m) = out_rx.recv() => {
                let m = match m {
                    Msg::Text(t) => WsMessage::Text(t.into()),
                    Msg::Bin(b) => WsMessage::Binary(b.into()),
                };
                if sink.send(m).await.is_err() {
                    break Err("Lost the relay connection".to_string());
                }
            }
            m = stream.next() => {
                heard = Instant::now();
                match m {
                    Some(Ok(WsMessage::Text(t))) => {
                        if t.as_str() != "pong" && in_tx.send(Msg::Text(t.to_string())).await.is_err() {
                            break Err("Stopped serving phones".to_string());
                        }
                    }
                    Some(Ok(WsMessage::Binary(b))) => {
                        if in_tx.send(Msg::Bin(b.to_vec())).await.is_err() {
                            break Err("Stopped serving phones".to_string());
                        }
                    }
                    Some(Ok(WsMessage::Close(f))) => break Err(format!("The relay closed the connection{}", f.map(|f| format!(": {}", f.reason.as_str())).unwrap_or_default())),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Err(format!("Lost the relay connection: {e}")),
                    None => break Err("Lost the relay connection".to_string()),
                }
            }
            _ = ping.tick() => {
                if heard.elapsed() > DEAD_AFTER {
                    break Err("The relay stopped answering".to_string());
                }
                if sink.send(WsMessage::Text("ping".into())).await.is_err() {
                    break Err("Lost the relay connection".to_string());
                }
            }
        }
    };
    serving.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
    res
}

// ---------------------------------------------------------------- relay messages

/// A relay message (CLOUD.md, "Relay").
#[derive(Debug)]
pub(crate) enum Msg {
    Text(String),
    Bin(Vec<u8>),
}

struct Conn {
    inbox: mpsc::Sender<Vec<u8>>,
    task: tokio::task::JoinHandle<()>,
}

/// A message waiting for the relay socket. Sealed messages are sealed only when they leave, so
/// they get their counters in the order they are sent whichever lane they waited in.
enum Item {
    Msg(Msg),
    Seal { c: u32, seal: Arc<Mutex<Sealer>>, plain: Vec<u8> },
}

impl Item {
    fn into_msg(self) -> Msg {
        match self {
            Item::Msg(m) => m,
            Item::Seal { c, seal, plain } => {
                let frame = seal.lock().unwrap_or_else(|e| e.into_inner()).seal(&plain);
                let mut v = Vec::with_capacity(4 + frame.len());
                v.extend_from_slice(&c.to_be_bytes());
                v.extend_from_slice(&frame);
                Msg::Bin(v)
            }
        }
    }
}

/// The two lanes to the relay: control messages and small responses (never wait), and bulk data
/// (one chunk at a time, so it never piles up in front of them).
#[derive(Clone)]
struct Lanes {
    ctl: mpsc::UnboundedSender<Item>,
    bulk: mpsc::Sender<Item>,
}

/// Feeds the relay socket, control lane first.
async fn writer(mut ctl: mpsc::UnboundedReceiver<Item>, mut bulk: mpsc::Receiver<Item>, tx: mpsc::Sender<Msg>) {
    loop {
        let item = tokio::select! {
            biased;
            Some(i) = ctl.recv() => i,
            Some(i) = bulk.recv() => i,
            else => return,
        };
        if tx.send(item.into_msg()).await.is_err() {
            return;
        }
    }
}

/// Sends to one phone through the relay.
#[derive(Clone)]
struct Out {
    lanes: Lanes,
    c: u32,
}

impl Out {
    fn frame(&self, kind: u8, body: &[u8]) {
        let mut v = Vec::with_capacity(5 + body.len());
        v.extend_from_slice(&self.c.to_be_bytes());
        v.push(kind);
        v.extend_from_slice(body);
        let _ = self.lanes.ctl.send(Item::Msg(Msg::Bin(v)));
    }

    fn refuse(&self, code: &str, error: &str) {
        self.frame(cc::KIND_REFUSE, json!({ "code": code, "error": error }).to_string().as_bytes());
        self.close();
    }

    fn close(&self) {
        let _ = self.lanes.ctl.send(Item::Msg(Msg::Text(json!({ "t": "close", "c": self.c }).to_string())));
    }
}

/// Runs until the relay connection ends: one session task per connected phone. `tx` is the relay
/// socket's (short) queue.
pub(crate) async fn serve(mut rx: mpsc::Receiver<Msg>, tx: mpsc::Sender<Msg>, router: Router) {
    let (ctl, ctl_rx) = mpsc::unbounded_channel();
    let (bulk, bulk_rx) = mpsc::channel(1);
    let lanes = Lanes { ctl, bulk };
    let writing = tokio::spawn(writer(ctl_rx, bulk_rx, tx));
    let mut conns: HashMap<u32, Conn> = HashMap::new();
    while let Some(m) = rx.recv().await {
        conns.retain(|_, c| !c.task.is_finished());
        match m {
            Msg::Text(t) => {
                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                let Some(c) = v["c"].as_u64().and_then(|c| u32::try_from(c).ok()) else { continue };
                match v["t"].as_str() {
                    Some("open") => {
                        if let Some(old) = conns.remove(&c) {
                            old.task.abort();
                        }
                        let out = Out { lanes: lanes.clone(), c };
                        if conns.len() >= MAX_PHONES {
                            out.refuse("busy", "Too many phones are connected to this PC right now.");
                            continue;
                        }
                        let (itx, irx) = mpsc::channel(INBOX);
                        let task = tokio::spawn(session(irx, out, router.clone()));
                        conns.insert(c, Conn { inbox: itx, task });
                    }
                    Some("gone") => {
                        if let Some(old) = conns.remove(&c) {
                            old.task.abort();
                        }
                    }
                    _ => {}
                }
            }
            Msg::Bin(b) => {
                if b.len() < 5 {
                    continue;
                }
                let c = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                let Some(conn) = conns.get(&c) else { continue };
                // Too big, or a phone sending faster than its session reads: cut it off.
                if b.len() - 4 > MAX_FRAME || conn.inbox.try_send(b[4..].to_vec()).is_err() {
                    Out { lanes: lanes.clone(), c }.close();
                    if let Some(old) = conns.remove(&c) {
                        old.task.abort();
                    }
                }
            }
        }
    }
    for (_, c) in conns {
        c.task.abort();
    }
    writing.abort();
}

// ---------------------------------------------------------------- sessions

/// Sends sealed messages to one phone. They are sealed as they leave (see [`Item`]); each lane
/// keeps its order, so a stream sends all its body on one lane and its head before its body.
#[derive(Clone)]
struct Tx {
    out: Out,
    seal: Arc<Mutex<Sealer>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Lane {
    Ctl,
    Bulk,
}

fn data_msg(s: u32, bytes: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(5 + bytes.len());
    p.push(cc::MSG_DATA);
    p.extend_from_slice(&s.to_be_bytes());
    p.extend_from_slice(bytes);
    p
}

fn end_msg(s: u32) -> Vec<u8> {
    let mut p = vec![cc::MSG_END];
    p.extend_from_slice(&s.to_be_bytes());
    p
}

fn json_msg(v: Value) -> Vec<u8> {
    let mut p = vec![cc::MSG_JSON];
    p.extend_from_slice(v.to_string().as_bytes());
    p
}

impl Tx {
    fn item(&self, plain: Vec<u8>) -> Item {
        Item::Seal { c: self.out.c, seal: self.seal.clone(), plain }
    }

    /// On the control lane: never waits.
    fn ctl(&self, plain: Vec<u8>) {
        let _ = self.out.lanes.ctl.send(self.item(plain));
    }

    async fn send(&self, lane: Lane, plain: Vec<u8>) {
        match lane {
            Lane::Ctl => self.ctl(plain),
            Lane::Bulk => {
                let _ = self.out.lanes.bulk.send(self.item(plain)).await;
            }
        }
    }

    fn json(&self, v: Value) {
        self.ctl(json_msg(v));
    }
}

#[derive(Deserialize)]
struct Hello {
    v: u32,
    #[serde(default)]
    e: String,
    #[serde(default)]
    d: String,
    #[serde(default)]
    n: String,
    #[serde(default)]
    id: String,
}

const UPDATE_PAGE: &str = "Reload this page: MangaVox on your PC speaks a different version.";
const PAIR_GONE: &str = "This code expired or was already used. Show a new code on your PC and scan it again.";
const REMOVED: &str = "This phone was removed on your PC. Scan the code in MangaVox → Mobile again.";

/// One phone: handshake, approval for a new phone, then requests until either side goes away.
async fn session(mut inbox: mpsc::Receiver<Vec<u8>>, out: Out, router: Router) {
    let Some(id) = identity() else {
        out.refuse("busy", "Cloud access is not ready on your PC.");
        return;
    };
    let Ok(Some(first)) = tokio::time::timeout(HANDSHAKE_WAIT, inbox.recv()).await else {
        out.close();
        return;
    };
    let kind = first.first().copied().unwrap_or(0);
    let Some(hello) = serde_json::from_slice::<Hello>(first.get(1..).unwrap_or_default()).ok().filter(|h| h.v == 1) else {
        out.refuse("bad", UPDATE_PAGE);
        return;
    };
    let (Some(ep), Some(np)) = (cc::parse_point(&hello.e), URL_SAFE_NO_PAD.decode(&hello.n).ok().filter(|n| n.len() == 16)) else {
        out.refuse("bad", UPDATE_PAGE);
        return;
    };
    let (d, salt, code, known) = match kind {
        cc::KIND_PAIR => {
            let Some(code) = auth::cloud_code_live(Instant::now()) else {
                out.refuse("pair", PAIR_GONE);
                return;
            };
            let salt: Option<[u8; 32]> = URL_SAFE_NO_PAD.decode(&code).ok().and_then(|b| b.try_into().ok());
            let (Some(d), Some(salt)) = (cc::parse_point(&hello.d), salt) else {
                out.refuse("bad", UPDATE_PAGE);
                return;
            };
            (d, salt, Some(code), None)
        }
        cc::KIND_RESUME => {
            let Some((dev, d)) = auth::cloud_device(&hello.id).and_then(|(dev, key)| Some((dev, cc::parse_point(&key)?))) else {
                out.refuse("unknown", REMOVED);
                return;
            };
            (d, cc::RESUME_SALT, None, Some(dev))
        }
        _ => {
            out.refuse("bad", UPDATE_PAGE);
            return;
        }
    };

    let ec = EphemeralSecret::random(&mut OsRng);
    let ecp = ec.public_key();
    let mut nc = [0u8; 16];
    OsRng.fill_bytes(&mut nc);
    let s = id.key.public_key();
    let t = Transcript { kind, room: &id.room_bytes, ep: &ep, ec: &ecp, s: &s, d: &d, np: &np, nc: &nc };
    let (mut opener, sealer) = cc::pc_session(&ec, &id.key, &salt, &t);
    let verify = t.verify_code();
    drop(ec);
    out.frame(cc::KIND_REPLY, json!({ "v": 1, "e": cc::b64(&cc::point(&ecp)), "s": cc::b64(&cc::point(&s)), "n": cc::b64(&nc) }).to_string().as_bytes());

    let Ok(Some(fin)) = tokio::time::timeout(HANDSHAKE_WAIT, inbox.recv()).await else {
        out.close();
        return;
    };
    let fin = open_json(&mut opener, &fin).filter(|v| v["t"] == "fin");
    let Some(fin) = fin else {
        if code.is_some() {
            auth::record_cloud_fail(Instant::now());
            out.refuse("pair", PAIR_GONE);
        } else {
            out.refuse("unknown", REMOVED);
        }
        return;
    };
    let tx = Tx { out: out.clone(), seal: Arc::new(Mutex::new(sealer)) };

    let dev = match (code, known) {
        (Some(code), _) => {
            // One question per code: it is used up before the PC asks.
            if !auth::take_cloud_code(&code, Instant::now()) {
                out.refuse("pair", PAIR_GONE);
                return;
            }
            let ua: String = fin["ua"].as_str().unwrap_or("").chars().take(512).collect();
            let name = auth::device_name(&ua);
            let (ask_id, mut answer) = ask(name.clone(), verify);
            let _guard = AskGuard(ask_id);
            tx.json(json!({ "t": "wait" }));
            eprintln!("[cloud] {name} asks to connect; waiting for an answer on the Mobile page");
            let deadline = tokio::time::sleep(APPROVAL_WAIT);
            tokio::pin!(deadline);
            let allowed = loop {
                tokio::select! {
                    a = &mut answer => break a.unwrap_or(false),
                    _ = &mut deadline => break false,
                    // Only the phone leaving, or its own sealed "cancel", ends the wait: a frame
                    // the relay slips in does not open and is ignored.
                    f = inbox.recv() => match f {
                        None => break false,
                        Some(f) => {
                            if open_json(&mut opener, &f).is_some_and(|v| v["t"] == "cancel") {
                                break false;
                            }
                        }
                    },
                }
            };
            if !allowed {
                out.refuse("denied", "Not allowed on your PC. Scan the code again to ask once more.");
                return;
            }
            auth::add_cloud_device(name, hello.d.clone())
        }
        (None, Some(dev)) => dev,
        (None, None) => return,
    };
    tx.json(json!({ "t": "ok", "id": dev.id, "name": dev.name, "pc": pc_name(), "version": env!("CARGO_PKG_VERSION") }));
    eprintln!("[cloud] {} ({}) connected", dev.name, dev.id);
    requests(inbox, tx, opener, dev, router).await;
}

/// A sealed JSON message, or `None` when it does not open or is not JSON.
fn open_json(opener: &mut Opener, frame: &[u8]) -> Option<Value> {
    let p = opener.open(frame)?;
    (p.first() == Some(&cc::MSG_JSON)).then(|| serde_json::from_slice(&p[1..]).ok()).flatten()
}

/// Tunnelled requests of one authenticated phone.
async fn requests(mut inbox: mpsc::Receiver<Vec<u8>>, tx: Tx, mut opener: Opener, dev: Device, router: Router) {
    // Stream id -> (credit, task, generation). A finished task only removes its own generation,
    // so an id the phone reuses after a cancel is never dropped from the count early.
    let mut streams: HashMap<u32, (Arc<Semaphore>, tokio::task::AbortHandle, u64)> = HashMap::new();
    let mut tasks: JoinSet<(u32, u64)> = JoinSet::new();
    let mut generation = 0u64;
    let mut check = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            f = inbox.recv() => {
                let Some(f) = f else { break };
                let Some(p) = opener.open(&f) else {
                    eprintln!("[cloud] {}: a message failed to open; closing", dev.id);
                    tx.out.close();
                    break;
                };
                if p.first() != Some(&cc::MSG_JSON) {
                    continue;
                }
                let Ok(v) = serde_json::from_slice::<Value>(&p[1..]) else { continue };
                let Some(s) = v["s"].as_u64().and_then(|s| u32::try_from(s).ok()) else { continue };
                match v["t"].as_str() {
                    Some("req") => {
                        if streams.contains_key(&s) {
                            tx.out.close();
                            break;
                        }
                        // Removed on the PC: no more requests (the guard refuses them too).
                        if auth::cloud_device(&dev.id).is_none() {
                            tx.out.close();
                            break;
                        }
                        let req = (streams.len() < MAX_STREAMS).then(|| build_request(&v, &dev.id));
                        let Some(Some(req)) = req else {
                            let st = if streams.len() >= MAX_STREAMS { 503 } else { 400 };
                            tx.json(json!({ "t": "head", "s": s, "st": st, "h": { "content-type": "application/json" } }));
                            tx.ctl(data_msg(s, json!({ "error": if st == 503 { "busy" } else { "bad request" } }).to_string().as_bytes()));
                            tx.ctl(end_msg(s));
                            continue;
                        };
                        let credit = Arc::new(Semaphore::new(WINDOW));
                        generation += 1;
                        let g = generation;
                        let (router, tx, c) = (router.clone(), tx.clone(), credit.clone());
                        let handle = tasks.spawn(async move { (stream(s, req, router, tx, c).await, g) });
                        streams.insert(s, (credit, handle, g));
                    }
                    Some("ack") => {
                        if let Some((credit, _, _)) = streams.get(&s) {
                            let n = v["n"].as_u64().unwrap_or(0).min(WINDOW as u64) as usize;
                            if credit.available_permits() + n <= 2 * WINDOW {
                                credit.add_permits(n);
                            }
                        }
                    }
                    Some("cancel") => {
                        if let Some((_, h, _)) = streams.remove(&s) {
                            h.abort();
                        }
                    }
                    _ => {}
                }
            }
            Some(done) = tasks.join_next(), if !tasks.is_empty() => {
                if let Ok((s, g)) = done {
                    if streams.get(&s).is_some_and(|e| e.2 == g) {
                        streams.remove(&s);
                    }
                }
            }
            _ = check.tick() => {
                if auth::cloud_device(&dev.id).is_none() {
                    eprintln!("[cloud] {} was removed; closing its session", dev.id);
                    tx.out.close();
                    break;
                }
            }
        }
    }
    tasks.abort_all();
}

/// The request a `req` message describes, or `None` when it is malformed.
fn build_request(v: &Value, device: &str) -> Option<Request<Body>> {
    let method = Method::from_bytes(v["m"].as_str()?.as_bytes()).ok().filter(|m| matches!(*m, Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::DELETE | Method::PATCH))?;
    let path = v["p"].as_str()?;
    if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') || path.len() > 4096 {
        return None;
    }
    let uri: Uri = path.parse().ok()?;
    if uri.scheme().is_some() || uri.authority().is_some() {
        return None;
    }
    let body = match v["b"].as_str() {
        Some(b) => STANDARD.decode(b).ok().filter(|b| b.len() <= MAX_BODY)?,
        None => Vec::new(),
    };
    let mut r = Request::builder().method(method).uri(uri).header(header::HOST, "cloud");
    if let Some(h) = v["h"].as_object() {
        for (k, val) in h {
            let k = k.to_ascii_lowercase();
            if let (true, Some(val)) = (REQ_HEADERS.contains(&k.as_str()), val.as_str().and_then(|s| HeaderValue::from_str(s).ok())) {
                r = r.header(k.as_str(), val);
            }
        }
    }
    r.extension(CloudPeer { device: device.to_string() }).body(Body::from(body)).ok()
}

/// Runs one request and sends its response in chunks, at most [`WINDOW`] bytes ahead of the
/// phone's acks. Returns the stream id.
async fn stream(s: u32, req: Request<Body>, router: Router, tx: Tx, credit: Arc<Semaphore>) -> u32 {
    let res = match router.oneshot(req).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    let (parts, mut body) = res.into_parts();
    let mut h = serde_json::Map::new();
    for name in RESP_HEADERS {
        if let Some(v) = parts.headers.get(*name).and_then(|v| v.to_str().ok()) {
            h.insert((*name).to_string(), Value::String(v.to_string()));
        }
    }
    tx.json(json!({ "t": "head", "s": s, "st": parts.status.as_u16(), "h": h }));
    // An API answer or an icon jumps the queue; images and video wait their turn behind it.
    let lane = if body.size_hint().exact().is_some_and(|n| n <= SMALL) { Lane::Ctl } else { Lane::Bulk };
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(f) => {
                let Ok(data) = f.into_data() else { continue };
                for chunk in data.chunks(CHUNK) {
                    match credit.acquire_many(chunk.len() as u32).await {
                        Ok(p) => p.forget(),
                        Err(_) => return s,
                    }
                    tx.send(lane, data_msg(s, chunk)).await;
                }
            }
            Err(e) => {
                tx.send(lane, json_msg(json!({ "t": "err", "s": s, "error": e.to_string() }))).await;
                return s;
            }
        }
    }
    tx.send(lane, end_msg(s)).await;
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_crypto::{phone_session, KIND_PAIR, KIND_REFUSE, KIND_REPLY, KIND_RESUME, KIND_SEALED, RESUME_SALT};

    struct Relay {
        to_pc: mpsc::Sender<Msg>,
        from_pc: mpsc::Receiver<Msg>,
        task: tokio::task::JoinHandle<()>,
    }

    fn relay() -> Relay {
        let (to_pc, rx) = mpsc::channel(64);
        let (tx, from_pc) = mpsc::channel(OUTBOX);
        let task = tokio::spawn(serve(rx, tx, crate::server::router()));
        Relay { to_pc, from_pc, task }
    }

    impl Relay {
        async fn open(&self, c: u32) {
            self.to_pc.send(Msg::Text(json!({ "t": "open", "c": c }).to_string())).await.unwrap();
        }

        async fn send(&self, c: u32, frame: &[u8]) {
            let mut v = c.to_be_bytes().to_vec();
            v.extend_from_slice(frame);
            self.to_pc.send(Msg::Bin(v)).await.unwrap();
        }

        /// The next frame for phone `c`; `None` when the PC closes it.
        async fn recv(&mut self, c: u32) -> Option<Vec<u8>> {
            loop {
                let m = tokio::time::timeout(Duration::from_secs(10), self.from_pc.recv()).await.expect("PC answered").expect("channel open");
                match m {
                    Msg::Bin(b) if b[..4] == c.to_be_bytes() => return Some(b[4..].to_vec()),
                    Msg::Text(t) if t.contains("\"close\"") && t.contains(&format!("\"c\":{c}")) => return None,
                    _ => {}
                }
            }
        }
    }

    /// A phone speaking the protocol (what `tunnel.js` does).
    struct Phone {
        d: SecretKey,
        open: Opener,
        seal: Sealer,
        /// The code this phone would show while waiting for Allow.
        verify: String,
    }

    fn hello(kind: u8, d: &SecretKey, id: &str) -> (EphemeralSecret, [u8; 16], Vec<u8>) {
        let ep = EphemeralSecret::random(&mut OsRng);
        let mut np = [0u8; 16];
        OsRng.fill_bytes(&mut np);
        let body = json!({ "v": 1, "e": cc::b64(&cc::point(&ep.public_key())), "d": cc::b64(&cc::point(&d.public_key())), "n": cc::b64(&np), "id": id });
        let mut f = vec![kind];
        f.extend_from_slice(body.to_string().as_bytes());
        (ep, np, f)
    }

    /// Sends PAIR / RESUME and finishes the handshake with `salt`; returns the phone and the REPLY.
    async fn handshake(r: &mut Relay, c: u32, kind: u8, d: SecretKey, id: &str, salt: [u8; 32], room: &[u8]) -> Result<Phone, Value> {
        let (ep, np, f) = hello(kind, &d, id);
        r.send(c, &f).await;
        let reply = r.recv(c).await.expect("a reply");
        if reply[0] == KIND_REFUSE {
            return Err(serde_json::from_slice(&reply[1..]).unwrap());
        }
        assert_eq!(reply[0], KIND_REPLY);
        let v: Value = serde_json::from_slice(&reply[1..]).unwrap();
        let ec = cc::parse_point(v["e"].as_str().unwrap()).unwrap();
        let s = cc::parse_point(v["s"].as_str().unwrap()).unwrap();
        let nc = URL_SAFE_NO_PAD.decode(v["n"].as_str().unwrap()).unwrap();
        let (epp, dp) = (ep.public_key(), d.public_key());
        let t = Transcript { kind, room, ep: &epp, ec: &ec, s: &s, d: &dp, np: &np, nc: &nc };
        let (open, seal) = phone_session(&ep, &d, &salt, &t);
        let mut p = Phone { d, open, seal, verify: t.verify_code() };
        p.json(r, c, json!({ "t": "fin", "ua": "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) Version/17.0 Mobile Safari/604.1" })).await;
        Ok(p)
    }

    impl Phone {
        async fn json(&mut self, r: &Relay, c: u32, v: Value) {
            let mut p = vec![cc::MSG_JSON];
            p.extend_from_slice(v.to_string().as_bytes());
            r.send(c, &self.seal.seal(&p)).await;
        }

        /// The next sealed JSON message, or the REFUSE body.
        async fn next(&mut self, r: &mut Relay, c: u32) -> Option<Value> {
            let f = r.recv(c).await?;
            if f[0] == KIND_REFUSE {
                return Some(json!({ "refused": serde_json::from_slice::<Value>(&f[1..]).unwrap() }));
            }
            assert_eq!(f[0], KIND_SEALED);
            let p = self.open.open(&f).expect("opens");
            assert_eq!(p[0], cc::MSG_JSON);
            Some(serde_json::from_slice(&p[1..]).unwrap())
        }

        /// Sends a request as the app does (with its CSRF header) and collects (status, body).
        async fn get(&mut self, r: &mut Relay, c: u32, s: u32, method: &str, path: &str) -> (u16, String) {
            self.req(r, c, s, method, path, json!({ "x-mangavox": "1" })).await
        }

        /// Sends a request and collects (status, body), acking as it reads.
        async fn req(&mut self, r: &mut Relay, c: u32, s: u32, method: &str, path: &str, h: Value) -> (u16, String) {
            self.json(r, c, json!({ "t": "req", "s": s, "m": method, "p": path, "h": h })).await;
            let mut status = 0;
            let mut body = Vec::new();
            loop {
                let f = r.recv(c).await.expect("response");
                let p = self.open.open(&f).expect("opens");
                match p[0] {
                    cc::MSG_JSON => {
                        let v: Value = serde_json::from_slice(&p[1..]).unwrap();
                        if v["t"] == "head" {
                            status = v["st"].as_u64().unwrap() as u16;
                        }
                    }
                    cc::MSG_DATA => {
                        body.extend_from_slice(&p[5..]);
                        let n = p.len() - 5;
                        self.json(r, c, json!({ "t": "ack", "s": s, "n": n })).await;
                    }
                    cc::MSG_END => return (status, String::from_utf8_lossy(&body).to_string()),
                    _ => panic!("unknown kind"),
                }
            }
        }
    }

    fn setup(tag: &str) -> (Identity, PathBuf) {
        let store = auth::tests::temp_store(tag);
        auth::clear_cloud_code();
        let id = Identity::generate();
        use_identity(id.clone());
        (id, store)
    }

    /// Answers the next question on the Mobile page.
    fn answer_soon(allow: bool) {
        tokio::spawn(async move {
            for _ in 0..200 {
                if let Some(p) = pending().first() {
                    answer(p.id, allow);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
    }

    #[tokio::test]
    async fn pairs_after_approval_then_tunnels_and_resumes() {
        let _g = auth::tests::serial();
        let (id, store) = setup("cloud-pair");
        let mut r = relay();
        let code = auth::new_cloud_code();
        let salt: [u8; 32] = URL_SAFE_NO_PAD.decode(&code).unwrap().try_into().unwrap();

        r.open(1).await;
        let mut p = handshake(&mut r, 1, KIND_PAIR, SecretKey::random(&mut OsRng), "", salt, &id.room_bytes).await.expect("paired");
        assert_eq!(p.next(&mut r, 1).await.unwrap()["t"], "wait");
        // A frame the relay slips in while the PC asks is ignored, not taken as "deny".
        r.send(1, &[cc::KIND_SEALED, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]).await;
        // Let the session read it while it still waits (after Allow, a frame that does not open
        // rightly ends the session).
        tokio::time::sleep(Duration::from_millis(300)).await;
        let asking = pending();
        assert_eq!(asking.len(), 1);
        assert_eq!(asking[0].code, p.verify, "the PC shows the code the phone shows");
        answer(asking[0].id, true);
        let ok = p.next(&mut r, 1).await.unwrap();
        assert_eq!(ok["t"], "ok", "{ok}");
        assert_eq!(ok["name"], "iPhone · Safari");
        let dev_id = ok["id"].as_str().unwrap().to_string();
        assert!(auth::devices().iter().any(|d| d.id == dev_id && d.cloud && !d.control));
        assert!(auth::cloud_code_live(Instant::now()).is_none(), "the code is used up");

        let (st, body) = p.get(&mut r, 1, 1, "GET", "/api/me").await;
        assert_eq!(st, 200, "{body}");
        assert!(body.contains(&dev_id), "{body}");
        // The PC's own app page loads through the tunnel.
        let (st, body) = p.get(&mut r, 1, 2, "GET", "/").await;
        assert_eq!(st, 200);
        assert!(body.contains("/app.js"));
        // A state change without the app's CSRF header (a form from another site) is refused.
        let (st, _) = p.req(&mut r, 1, 20, "POST", "/api/logout", json!({})).await;
        assert_eq!(st, 403);
        assert!(auth::devices().iter().any(|d| d.id == dev_id), "still paired");
        // Remote control is off until allowed on the PC.
        let (st, _) = p.get(&mut r, 1, 3, "POST", "/api/mouse").await;
        assert_eq!(st, 403);
        let (st, _) = p.get(&mut r, 1, 4, "GET", "/api/screen/frame").await;
        assert_eq!(st, 403);
        // A URL that is not a plain path is refused.
        let (st, _) = p.get(&mut r, 1, 5, "GET", "//evil.example/x").await;
        assert_eq!(st, 400);
        let (st, _) = p.get(&mut r, 1, 6, "GET", "/pair?code=x").await;
        assert_eq!(st, 404);

        // A replayed frame ends the session.
        let replay = {
            let mut q = vec![cc::MSG_JSON];
            q.extend_from_slice(json!({ "t": "req", "s": 9, "m": "GET", "p": "/api/me", "h": {} }).to_string().as_bytes());
            p.seal.seal(&q)
        };
        r.send(1, &replay).await;
        let _ = p.next(&mut r, 1).await; // head
        while let Some(f) = r.recv(1).await {
            if p.open.open(&f).is_some_and(|q| q[0] == cc::MSG_END) {
                break;
            }
        }
        r.send(1, &replay).await;
        assert!(r.recv(1).await.is_none(), "the replay closes the session");

        // Resume with the device key.
        r.to_pc.send(Msg::Text(json!({ "t": "gone", "c": 1 }).to_string())).await.unwrap();
        r.open(2).await;
        let mut p2 = handshake(&mut r, 2, KIND_RESUME, p.d.clone(), &dev_id, RESUME_SALT, &id.room_bytes).await.expect("resumed");
        assert_eq!(p2.next(&mut r, 2).await.unwrap()["id"], dev_id);
        auth::set_control(&dev_id, true);
        let (st, _) = p2.get(&mut r, 2, 1, "GET", "/api/screen/frame").await;
        assert_ne!(st, 403, "control allowed now");

        // Another key for the same device id does not get in.
        r.open(3).await;
        let mut p3 = handshake(&mut r, 3, KIND_RESUME, SecretKey::random(&mut OsRng), &dev_id, RESUME_SALT, &id.room_bytes).await.expect("a reply");
        assert_eq!(p3.next(&mut r, 3).await.unwrap()["refused"]["code"], "unknown");

        // Removed on the PC: the session ends at its next request, and resuming fails.
        auth::revoke(&dev_id);
        p2.json(&r, 2, json!({ "t": "req", "s": 2, "m": "GET", "p": "/api/me", "h": {} })).await;
        assert!(r.recv(2).await.is_none());
        r.open(4).await;
        let refused = handshake(&mut r, 4, KIND_RESUME, p.d.clone(), &dev_id, RESUME_SALT, &id.room_bytes).await.err().expect("refused");
        assert_eq!(refused["code"], "unknown");

        r.task.abort();
        let _ = std::fs::remove_dir_all(store.parent().unwrap());
    }

    #[tokio::test]
    async fn wrong_code_denied_and_expired_pairings_fail() {
        let _g = auth::tests::serial();
        let (id, store) = setup("cloud-deny");
        let mut r = relay();

        // No code shown: refused at once.
        r.open(1).await;
        let e = handshake(&mut r, 1, KIND_PAIR, SecretKey::random(&mut OsRng), "", [0; 32], &id.room_bytes).await.err().expect("refused");
        assert_eq!(e["code"], "pair");

        // A wrong code: the fin does not open.
        let code = auth::new_cloud_code();
        r.open(2).await;
        let mut p = handshake(&mut r, 2, KIND_PAIR, SecretKey::random(&mut OsRng), "", [9; 32], &id.room_bytes).await.expect("a reply");
        let f = r.recv(2).await.unwrap();
        assert_eq!(f[0], KIND_REFUSE);
        assert!(p.open.open(&f).is_none());
        assert!(auth::cloud_code_live(Instant::now()).as_deref() == Some(code.as_str()), "a wrong guess does not burn the code");

        // Denied on the PC: no device, code used up.
        let salt: [u8; 32] = URL_SAFE_NO_PAD.decode(&code).unwrap().try_into().unwrap();
        r.open(3).await;
        answer_soon(false);
        let mut p = handshake(&mut r, 3, KIND_PAIR, SecretKey::random(&mut OsRng), "", salt, &id.room_bytes).await.expect("a reply");
        assert_eq!(p.next(&mut r, 3).await.unwrap()["t"], "wait");
        assert_eq!(p.next(&mut r, 3).await.unwrap()["refused"]["code"], "denied");
        assert!(auth::devices().is_empty());
        assert!(auth::cloud_code_live(Instant::now()).is_none());
        assert!(pending().is_empty());

        r.task.abort();
        let _ = std::fs::remove_dir_all(store.parent().unwrap());
    }

    #[test]
    fn links_and_urls() {
        let _g = auth::tests::serial();
        use_identity(Identity::generate());
        let link = pair_link("CODE", Some("http://192.168.1.5:8790/pair?code=x")).unwrap();
        assert!(link.starts_with(&format!("{}/#p=", base_url())), "{link}");
        assert!(link.ends_with("&l=http%3A%2F%2F192.168.1.5%3A8790%2Fpair%3Fcode%3Dx"), "{link}");
        assert_eq!(ws_url("https://app.bhippi.com/mangavox", "r").unwrap(), "wss://app.bhippi.com/mangavox/ws/pc?room=r");
        assert_eq!(ws_url("http://localhost:8787/mangavox", "r").unwrap(), "ws://localhost:8787/mangavox/ws/pc?room=r");
        assert!(ws_url("http://evil.example/mangavox", "r").is_err(), "plain http only on this machine");
        let id = Identity::generate();
        let back = Identity::from_stored(&id.stored()).unwrap();
        assert_eq!(back.key.to_bytes(), id.key.to_bytes());
        assert_eq!(back.room, id.room);
    }
}
