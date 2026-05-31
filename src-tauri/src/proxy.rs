//! Local reverse proxy that fronts the BeeAPI gateway for all configured
//! CLIs. CLIs point at either `http://127.0.0.1:<port>` or (if LAN mode is
//! enabled) `http://<lan-ip>:<port>`. The proxy:
//!
//! - validates an auto-generated local Bearer token on every incoming request,
//! - keeps a pool of user-supplied API keys,
//! - round-robins across healthy keys, retries on 401/403/429/5xx,
//! - records the last 300 requests in an in-memory ring for the UI "Sessions"
//!   tab.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub const DEFAULT_PORT: u16 = 17171;
pub const DEFAULT_UPSTREAM: &str = "https://beeapi.ai/";
pub const REQUEST_LOG_CAP: usize = 300;

// --------------------------------------------------------------------------
// Pool structures
// --------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub secret: String,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_enabled() -> bool {
    true
}

fn default_weight() -> u32 {
    1
}

fn estimate_cost(input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
    const INPUT_RATE: f64 = 0.000_000_15;
    const OUTPUT_RATE: f64 = 0.000_000_60;
    const CACHE_READ_RATE: f64 = 0.000_000_02;
    const CACHE_WRITE_RATE: f64 = 0.000_000_25;
    input as f64 * INPUT_RATE
        + output as f64 * OUTPUT_RATE
        + cache_read as f64 * CACHE_READ_RATE
        + cache_write as f64 * CACHE_WRITE_RATE
}

#[derive(Clone, Debug, Serialize)]
pub struct KeyStat {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub weight: u32,
    pub success: u64,
    pub failure: u64,
    pub last_status: Option<u16>,
    pub cooling_until: Option<i64>,
    pub cooldown_stage: u32,
    pub last_error: Option<String>,
    pub secret_tail: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub estimated_cost: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RequestLogEntry {
    pub id: u64,
    pub ts: i64,
    pub method: String,
    pub path: String,
    pub model: Option<String>,
    pub status: u16,
    pub key_label: String,
    pub key_id: String,
    pub latency_ms: u64,
    pub retries: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub usage_recorded: bool,
    pub upstream_error: Option<String>,
}

struct KeyRuntime {
    key: ApiKey,
    success: u64,
    failure: u64,
    consecutive_failures: u32,
    cooldown_stage: u32,
    last_status: Option<u16>,
    last_error: Option<String>,
    cooling_until: Option<Instant>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    estimated_cost: f64,
    current_weight: i64,
}

impl KeyRuntime {
    fn new(k: ApiKey) -> Self {
        Self {
            key: k,
            success: 0,
            failure: 0,
            consecutive_failures: 0,
            cooldown_stage: 0,
            last_status: None,
            last_error: None,
            cooling_until: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            estimated_cost: 0.0,
            current_weight: 0,
        }
    }

    fn available_at(&self, now: Instant) -> bool {
        self.key.enabled && self.cooling_until.map(|t| t <= now).unwrap_or(true)
    }

    fn weight_score(&self) -> i64 {
        self.key.weight.max(1) as i64
    }

    fn add_usage(&mut self, input: u64, output: u64, cache_read: u64, cache_write: u64) {
        self.input_tokens += input;
        self.output_tokens += output;
        self.cache_read_tokens += cache_read;
        self.cache_write_tokens += cache_write;
        self.estimated_cost += estimate_cost(input, output, cache_read, cache_write);
    }

    fn snapshot(&self) -> KeyStat {
        let secret = &self.key.secret;
        let tail = if secret.len() >= 6 {
            secret[secret.len() - 6..].to_string()
        } else {
            secret.clone()
        };
        KeyStat {
            id: self.key.id.clone(),
            label: self.key.label.clone(),
            enabled: self.key.enabled,
            weight: self.key.weight.max(1),
            success: self.success,
            failure: self.failure,
            last_status: self.last_status,
            cooling_until: self.cooling_until.map(|i| {
                let now = Instant::now();
                let remain = if i > now { (i - now).as_secs() } else { 0 };
                chrono::Utc::now().timestamp() + remain as i64
            }),
            cooldown_stage: self.cooldown_stage,
            last_error: self.last_error.clone(),
            secret_tail: tail,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            estimated_cost: self.estimated_cost,
        }
    }
}

pub struct Pool {
    keys: RwLock<Vec<KeyRuntime>>,
    cursor: RwLock<usize>,
    /// Preferred key used as the first routing target when available.
    preferred_key_id: RwLock<Option<String>>,
    upstream: RwLock<String>,
    /// Local bearer token CLIs must present.
    local_token: RwLock<String>,
    /// Whether the proxy is bound to 0.0.0.0 (LAN) vs 127.0.0.1 (host only).
    lan: RwLock<bool>,
    /// When false, the CLI configs are written with the upstream URL + the
    /// single "primary" key directly. The proxy still runs for use via
    /// UI tooling but CLIs bypass it.
    pool_enabled: RwLock<bool>,
    /// How many consecutive failures before a key gets cooled down.
    fail_threshold: RwLock<u32>,
    log: RwLock<VecDeque<RequestLogEntry>>,
    next_log_id: AtomicU64,
    /// Token usage counters (only tracked when pool is enabled).
    token_usage: RwLock<TokenUsage>,
}

/// Accumulated token usage counters.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TokenUsage {
    /// Input/prompt tokens in current session.
    pub session_input: u64,
    /// Output/completion tokens in current session.
    pub session_output: u64,
    /// Cache read tokens in current session.
    pub session_cache_read: u64,
    /// Cache write tokens in current session.
    pub session_cache_write: u64,
    /// Total input tokens across all sessions (persisted).
    pub total_input: u64,
    /// Total output tokens across all sessions (persisted).
    pub total_output: u64,
    /// Total cache read tokens across all sessions (persisted).
    pub total_cache_read: u64,
    /// Total cache write tokens across all sessions (persisted).
    pub total_cache_write: u64,
}

impl Pool {
    fn new(upstream: String, token: String, lan: bool, pool_enabled: bool) -> Self {
        Self {
            keys: RwLock::new(Vec::new()),
            cursor: RwLock::new(0),
            preferred_key_id: RwLock::new(None),
            upstream: RwLock::new(upstream),
            local_token: RwLock::new(token),
            lan: RwLock::new(lan),
            pool_enabled: RwLock::new(pool_enabled),
            fail_threshold: RwLock::new(1),
            log: RwLock::new(VecDeque::with_capacity(REQUEST_LOG_CAP)),
            next_log_id: AtomicU64::new(1),
            token_usage: RwLock::new(TokenUsage::default()),
        }
    }

    pub fn set_keys(&self, new_keys: Vec<ApiKey>) {
        let mut guard = self.keys.write();
        let existing: HashMap<String, KeyRuntime> =
            guard.drain(..).map(|r| (r.key.id.clone(), r)).collect();
        let mut existing = existing;
        let mut seen_secrets = HashSet::new();
        let mut merged = Vec::with_capacity(new_keys.len());
        for k in new_keys {
            let secret_key = k.secret.trim().to_string();
            if !secret_key.is_empty() && !seen_secrets.insert(secret_key) {
                continue;
            }
            if let Some(mut prev) = existing.remove(&k.id) {
                prev.key = k;
                merged.push(prev);
            } else {
                merged.push(KeyRuntime::new(k));
            }
        }
        *guard = merged;
        *self.cursor.write() = 0;
    }

    pub fn upstream(&self) -> String {
        self.upstream.read().clone()
    }

    pub fn local_token(&self) -> String {
        self.local_token.read().clone()
    }

    pub fn set_local_token(&self, token: String) {
        *self.local_token.write() = token;
    }

    pub fn lan(&self) -> bool {
        *self.lan.read()
    }

    pub fn set_lan(&self, lan: bool) {
        *self.lan.write() = lan;
    }

    pub fn pool_enabled(&self) -> bool {
        *self.pool_enabled.read()
    }

    pub fn set_pool_enabled(&self, enabled: bool) {
        *self.pool_enabled.write() = enabled;
    }

    pub fn set_preferred_key_id(&self, id: Option<String>) {
        *self.preferred_key_id.write() = id;
    }

    pub fn fail_threshold(&self) -> u32 {
        *self.fail_threshold.read()
    }

    pub fn set_fail_threshold(&self, threshold: u32) {
        *self.fail_threshold.write() = threshold.max(1);
    }

    pub fn stats(&self) -> Vec<KeyStat> {
        self.keys.read().iter().map(|r| r.snapshot()).collect()
    }

    pub fn raw_keys(&self) -> Vec<ApiKey> {
        self.keys.read().iter().map(|r| r.key.clone()).collect()
    }

    #[allow(dead_code)]
    pub fn first_enabled_secret(&self) -> Option<String> {
        self.keys
            .read()
            .iter()
            .find(|r| r.key.enabled)
            .map(|r| r.key.secret.clone())
    }

    pub fn recent_log(&self) -> Vec<RequestLogEntry> {
        self.log.read().iter().cloned().collect()
    }

    pub fn alloc_log_id(&self) -> u64 {
        self.next_log_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn clear_log(&self) {
        self.log.write().clear();
    }

    pub fn record_request_usage(
        &self,
        id: u64,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> bool {
        let mut guard = self.log.write();
        if let Some(entry) = guard.iter_mut().rev().find(|e| e.id == id) {
            if entry.usage_recorded {
                return false;
            }
            entry.input_tokens = input;
            entry.output_tokens = output;
            entry.cache_read_tokens = cache_read;
            entry.cache_write_tokens = cache_write;
            entry.usage_recorded = true;
            return true;
        }
        false
    }

    /// Add tokens to the usage counters.
    pub fn add_usage(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) {
        let mut usage = self.token_usage.write();
        usage.session_input += input;
        usage.session_output += output;
        usage.session_cache_read += cache_read;
        usage.session_cache_write += cache_write;
        usage.total_input += input;
        usage.total_output += output;
        usage.total_cache_read += cache_read;
        usage.total_cache_write += cache_write;
    }

    pub fn add_key_usage(&self, idx: usize, input: u64, output: u64, cache_read: u64, cache_write: u64) {
        let mut guard = self.keys.write();
        if let Some(rt) = guard.get_mut(idx) {
            rt.add_usage(input, output, cache_read, cache_write);
        }
    }

    /// Get current token usage snapshot.
    pub fn token_usage(&self) -> TokenUsage {
        self.token_usage.read().clone()
    }

    /// Set the totals from persisted storage on startup.
    pub fn set_persisted_totals(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) {
        let mut usage = self.token_usage.write();
        usage.total_input = input;
        usage.total_output = output;
        usage.total_cache_read = cache_read;
        usage.total_cache_write = cache_write;
    }

    /// Reset all token usage counters to zero.
    pub fn reset_usage(&self) {
        *self.token_usage.write() = TokenUsage::default();
    }

    fn push_log(&self, entry: RequestLogEntry) {
        let mut guard = self.log.write();
        if guard.len() >= REQUEST_LOG_CAP {
            guard.pop_front();
        }
        guard.push_back(entry);
    }

    fn order(&self) -> Vec<usize> {
        let now = Instant::now();
        let mut guard = self.keys.write();
        let n = guard.len();
        if n == 0 {
            return Vec::new();
        }

        let preferred = self.preferred_key_id.read().clone();
        let mut eligible: Vec<usize> = guard
            .iter()
            .enumerate()
            .filter(|(_, k)| k.available_at(now))
            .map(|(idx, _)| idx)
            .collect();

        if eligible.is_empty() {
            eligible = guard
                .iter()
                .enumerate()
                .filter(|(_, k)| k.key.enabled)
                .map(|(idx, _)| idx)
                .collect();
        }
        if eligible.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::with_capacity(eligible.len());
        if let Some(pref_id) = preferred {
            if let Some(pos) = eligible
                .iter()
                .position(|&idx| guard[idx].key.id == pref_id)
            {
                out.push(eligible.remove(pos));
            }
        }

        if eligible.is_empty() {
            return out;
        }

        let total_weight: i64 = eligible.iter().map(|&idx| guard[idx].weight_score()).sum();
        if total_weight <= 0 {
            for idx in eligible {
                out.push(idx);
            }
            return out;
        }

        for _ in 0..eligible.len() {
            for &idx in &eligible {
                guard[idx].current_weight += guard[idx].weight_score();
            }
            let Some(best) = eligible
                .iter()
                .copied()
                .max_by_key(|&idx| guard[idx].current_weight)
            else {
                break;
            };
            guard[best].current_weight -= total_weight;
            out.push(best);
        }
        out
    }

    fn record(&self, idx: usize, ok: bool, status: Option<u16>, err: Option<String>) {
        let threshold = *self.fail_threshold.read();
        let mut guard = self.keys.write();
        if idx >= guard.len() {
            return;
        }
        let rt = &mut guard[idx];
        let key_id = rt.key.id.clone();
        rt.last_status = status;
        if ok {
            rt.success += 1;
            rt.last_error = None;
            rt.cooling_until = None;
            rt.consecutive_failures = 0;
            rt.cooldown_stage = 0;
            drop(guard);
            let mut preferred = self.preferred_key_id.write();
            if preferred.is_none() {
                *preferred = Some(key_id);
            }
        } else {
            rt.failure += 1;
            rt.consecutive_failures += 1;
            rt.last_error = err;
            // Only cool down after reaching the threshold.
            if rt.consecutive_failures >= threshold {
                let stage = rt.consecutive_failures - threshold + 1;
                rt.cooldown_stage = stage;
                let secs = match stage {
                    1 => 3,
                    2 => 10,
                    3 => 30,
                    4 => 90,
                    _ => 300,
                };
                rt.cooling_until = Some(Instant::now() + Duration::from_secs(secs));
                rt.current_weight = 0;
            }
        }
    }

    fn key_at(&self, idx: usize) -> Option<(ApiKey, String)> {
        self.keys
            .read()
            .get(idx)
            .map(|r| (r.key.clone(), r.key.label.clone()))
    }
}

/// Handle exposed to the rest of the app.
#[derive(Clone)]
pub struct ProxyHandle {
    pub pool: Arc<Pool>,
    pub port: u16,
}

impl ProxyHandle {
    pub fn upstream(&self) -> String {
        self.pool.upstream()
    }

    pub fn local_base_for_cli(&self) -> String {
        // The CLI always uses 127.0.0.1 so that LAN reachability doesn't
        // depend on having a stable local IP address.
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn lan_base(&self) -> Option<String> {
        if !self.pool.lan() {
            return None;
        }
        let ip = local_ipv4().unwrap_or(Ipv4Addr::LOCALHOST);
        Some(format!("http://{}:{}", ip, self.port))
    }
}

// --------------------------------------------------------------------------
// Server wiring
// --------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    pool: Arc<Pool>,
    client: reqwest::Client,
}

pub struct StartOptions {
    pub upstream: String,
    pub token: String,
    pub lan: bool,
    pub pool_enabled: bool,
    pub port: u16,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            upstream: DEFAULT_UPSTREAM.to_string(),
            token: random_token(),
            lan: false,
            pool_enabled: true,
            port: DEFAULT_PORT,
        }
    }
}

pub fn random_token() -> String {
    // 32 hex chars == 128 bits of entropy.
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Fallback only if the OS RNG is unavailable.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(1);
        let mut s: u64 = (now as u64) ^ 0xdeadbeef_cafebabe;
        for b in &mut bytes {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *b = (s & 0xff) as u8;
        }
    }
    let mut out = String::with_capacity(32);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    format!("bees-{out}")
}

pub fn start(options: StartOptions) -> Result<ProxyHandle> {
    let pool = Arc::new(Pool::new(
        options.upstream,
        options.token,
        options.lan,
        options.pool_enabled,
    ));
    let (ready_tx, mut ready_rx) = mpsc::channel::<Result<u16, String>>(1);

    let pool_for_task = pool.clone();
    let port_hint = options.port;

    std::thread::Builder::new()
        .name("beeapi-proxy".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.blocking_send(Err(format!("启动 tokio runtime 失败: {e}")));
                    return;
                }
            };
            rt.block_on(async move {
                let client = match reqwest::Client::builder()
                    .user_agent("beeapi-switch-proxy/0.1")
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx
                            .send(Err(format!("创建 HTTP 客户端失败: {e}")))
                            .await;
                        return;
                    }
                };

                let state = AppState {
                    pool: pool_for_task.clone(),
                    client,
                };
                let app = Router::new().fallback(any(handle_any)).with_state(state);

                // Always bind on all interfaces and gate non-loopback clients
                // in the request handler. This lets the LAN switch take effect
                // immediately without restarting the proxy thread.
                let (bind_v4, bind_v6) = (
                    IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                );

                let addr_v4 = SocketAddr::new(bind_v4, port_hint);
                let listener_v4 = match tokio::net::TcpListener::bind(addr_v4).await {
                    Ok(l) => l,
                    Err(_) => {
                        // Port busy, try an ephemeral one.
                        let fallback = SocketAddr::new(bind_v4, 0);
                        match tokio::net::TcpListener::bind(fallback).await {
                            Ok(l) => l,
                            Err(e) => {
                                let _ = ready_tx
                                    .send(Err(format!(
                                        "监听 {addr_v4} 失败，回退到 {fallback} 也失败: {e}"
                                    )))
                                    .await;
                                return;
                            }
                        }
                    }
                };
                let bound = listener_v4
                    .local_addr()
                    .map(|a| a.port())
                    .unwrap_or(port_hint);
                let _ = ready_tx.send(Ok(bound)).await;

                let addr_v6 = SocketAddr::new(bind_v6, bound);
                let app_v4 = app.clone();
                let serve_v4 =
                    axum::serve(listener_v4, app_v4.into_make_service_with_connect_info::<SocketAddr>());

                let serve_v6 = match tokio::net::TcpListener::bind(addr_v6).await {
                    Ok(listener_v6) => {
                        let app_v6 = app.clone();
                        Some(axum::serve(
                            listener_v6,
                            app_v6.into_make_service_with_connect_info::<SocketAddr>(),
                        ))
                    }
                    Err(e) => {
                        eprintln!("[beeapi-proxy] IPv6 监听 {addr_v6} 失败: {e}");
                        None
                    }
                };

                if let Some(serve_v6) = serve_v6 {
                    let (res_v4, res_v6) = tokio::join!(serve_v4, serve_v6);
                    if let Err(e) = res_v4 {
                        eprintln!("[beeapi-proxy] serve v4 error: {e:#}");
                    }
                    if let Err(e) = res_v6 {
                        eprintln!("[beeapi-proxy] serve v6 error: {e:#}");
                    }
                } else if let Err(e) = serve_v4.await {
                    eprintln!("[beeapi-proxy] serve v4 error: {e:#}");
                }
            });
        })
        .context("无法启动代理线程")?;

    let bound = ready_rx
        .blocking_recv()
        .ok_or_else(|| anyhow!("代理线程未发回就绪信号"))?
        .map_err(|e| anyhow!(e))?;

    Ok(ProxyHandle { pool, port: bound })
}

// --------------------------------------------------------------------------
// Request handler
// --------------------------------------------------------------------------

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
    "authorization",
    "x-api-key",
];

fn strip_hop_headers(headers: &mut HeaderMap) {
    for h in HOP_BY_HOP {
        headers.remove(*h);
    }
    // Strip browser-specific headers to prevent upstream WAF/Cloudflare blocks (e.g. 403 Forbidden)
    headers.remove("origin");
    headers.remove("referer");
    headers.remove("sec-fetch-dest");
    headers.remove("sec-fetch-mode");
    headers.remove("sec-fetch-site");
    headers.remove("sec-ch-ua");
    headers.remove("sec-ch-ua-mobile");
    headers.remove("sec-ch-ua-platform");
    // Strip our custom key-selection header
    headers.remove("x-use-key-id");
}

/// Check the incoming `Authorization` / `x-api-key` headers against the
/// proxy's local token. Returns true when the caller is allowed.
fn check_local_auth(headers: &HeaderMap, expected: &str) -> bool {
    if expected.is_empty() {
        return true; // token disabled
    }
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        let stripped = v
            .strip_prefix("Bearer ")
            .or_else(|| v.strip_prefix("bearer "))
            .unwrap_or(v);
        if stripped == expected {
            return true;
        }
    }
    if let Some(v) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        if v.trim() == expected {
            return true;
        }
    }
    false
}

fn allowed_cors_origin(headers: &HeaderMap) -> Option<HeaderValue> {
    let origin = headers.get("origin")?.to_str().ok()?;
    let allowed = origin == "tauri://localhost"
        || origin == "http://tauri.localhost"
        || origin == "https://tauri.localhost"
        || origin == "http://127.0.0.1:1420"
        || origin == "http://localhost:1420";
    if allowed {
        HeaderValue::from_str(origin).ok()
    } else {
        None
    }
}

fn remote_allowed(addr: SocketAddr, pool: &Pool) -> bool {
    addr.ip().is_loopback() || pool.lan()
}

/// Inject CORS headers into a response for trusted app origins only.
fn inject_cors(resp: &mut Response, req_headers: &HeaderMap) {
    let Some(origin) = allowed_cors_origin(req_headers) else {
        return;
    };
    let h = resp.headers_mut();
    h.insert("access-control-allow-origin", origin);
    h.insert("vary", HeaderValue::from_static("Origin"));
    h.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD"),
    );
    h.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("Content-Type, Authorization, X-Api-Key, anthropic-version, x-use-key-id"),
    );
    h.insert(
        "access-control-expose-headers",
        HeaderValue::from_static("*"),
    );
    h.insert("access-control-max-age", HeaderValue::from_static("86400"));
}

async fn handle_any(
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
) -> Response {
    let start = Instant::now();
    let request_id = state.pool.alloc_log_id();
    let (parts, body) = req.into_parts();

    if !remote_allowed(remote_addr, &state.pool) {
        let mut resp = error_response(
            StatusCode::FORBIDDEN,
            "LAN access is disabled. Enable LAN mode in BeeAPI Switch before using this endpoint from another device.".into(),
        );
        inject_cors(&mut resp, &parts.headers);
        return resp;
    }

    // CORS preflight
    if parts.method == Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        inject_cors(&mut resp, &parts.headers);
        return resp;
    }

    // Health probe
    if parts.uri.path() == "/__beeapi/health" {
        let token = state.pool.local_token();
        let body = serde_json::json!({
            "ok": true,
            "upstream": state.pool.upstream(),
            "lan": state.pool.lan(),
            "has_token": !token.is_empty(),
        });
        let mut resp = (
            StatusCode::OK,
            [("content-type", "application/json")],
            body.to_string(),
        )
            .into_response();
        inject_cors(&mut resp, &parts.headers);
        return resp;
    }

    let expected = state.pool.local_token();
    if !check_local_auth(&parts.headers, &expected) {
        let entry = RequestLogEntry {
            id: request_id,
            ts: chrono::Utc::now().timestamp(),
            method: parts.method.to_string(),
            path: parts
                .uri
                .path_and_query()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "/".into()),
            status: 401,
            model: None,
            key_label: "-".into(),
            key_id: String::new(),
            latency_ms: start.elapsed().as_millis() as u64,
            retries: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            usage_recorded: false,
            upstream_error: Some("本地 Token 校验失败".into()),
        };
        state.pool.push_log(entry);
        let mut resp = error_response(
            StatusCode::UNAUTHORIZED,
            "本地代理 Token 校验失败。请使用 BeeAPI Switch 生成的 Token 访问。".into(),
        );
        inject_cors(&mut resp, &parts.headers);
        return resp;
    }

    let body_bytes = match axum::body::to_bytes(body, 50 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, format!("读取请求体失败: {e}")),
    };
    let model = extract_model_from_body(&body_bytes);

    let order = if let Some(id_val) = parts.headers.get("x-use-key-id").and_then(|v| v.to_str().ok()) {
        let guard = state.pool.keys.read();
        if let Some(pos) = guard.iter().position(|rt| rt.key.id == id_val) {
            vec![pos]
        } else {
            state.pool.order()
        }
    } else {
        state.pool.order()
    };

    if order.is_empty() {
        let entry = RequestLogEntry {
            id: state.pool.alloc_log_id(),
            ts: chrono::Utc::now().timestamp(),
            method: parts.method.to_string(),
            path: parts
                .uri
                .path_and_query()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "/".into()),
            status: 503,
            model,
            key_label: "-".into(),
            key_id: String::new(),
            latency_ms: start.elapsed().as_millis() as u64,
            retries: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            usage_recorded: false,
            upstream_error: Some("密钥池为空".into()),
        };
        state.pool.push_log(entry);
        let mut resp = error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "本地代理没有可用的 API Key。请在 BeeAPI Switch 中至少添加一把启用的密钥。".into(),
        );
        inject_cors(&mut resp, &parts.headers);
        return resp;
    }

    let upstream = state.pool.upstream();
    let mut last_status: Option<StatusCode> = None;
    let mut last_body: Option<bytes::Bytes> = None;
    let mut last_headers: Option<HeaderMap> = None;
    let mut retries: u32 = 0;
    let mut last_label = "-".to_string();
    let mut last_id = String::new();
    let mut last_err: Option<String> = None;

    for idx in order {
        let (key, label) = match state.pool.key_at(idx) {
            Some(k) => k,
            None => continue,
        };
        last_label = label.clone();
        last_id = key.id.clone();

        let url = match build_upstream_url(&upstream, &parts.uri) {
            Ok(u) => u,
            Err(e) => {
                return error_response(StatusCode::BAD_GATEWAY, format!("构造上游 URL 失败: {e}"))
            }
        };

        let method = reqwest_method(&parts.method);
        let mut forward_headers = parts.headers.clone();
        strip_hop_headers(&mut forward_headers);

        let bearer = format!("Bearer {}", key.secret);
        if let Ok(v) = HeaderValue::from_str(&bearer) {
            forward_headers.insert("authorization", v);
        }
        if let Ok(v) = HeaderValue::from_str(&key.secret) {
            forward_headers.insert(HeaderName::from_static("x-api-key"), v);
        }

        let modified_body = maybe_inject_stream_options(&body_bytes, &parts);

        // If body was modified, update content-length
        if modified_body.len() != body_bytes.len() {
            if let Ok(v) = HeaderValue::from_str(&modified_body.len().to_string()) {
                forward_headers.insert("content-length", v);
            }
        }

        let request = state
            .client
            .request(method, url)
            .headers(forward_headers)
            .body(modified_body);

        match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                let resp_headers = resp.headers().clone();

                if should_retry(status) {
                    // For retry-able errors, read the body to log it.
                    let bytes = resp.bytes().await.unwrap_or_default();
                    let preview_str = preview(&bytes);
                    state.pool.record(
                        idx,
                        false,
                        Some(status.as_u16()),
                        Some(format!("{} → {}", status.as_u16(), preview_str)),
                    );
                    last_status = Some(status);
                    last_body = Some(bytes);
                    last_headers = Some(resp_headers);
                    last_err = Some(format!("{} → {}", status.as_u16(), preview_str));
                    retries += 1;
                    continue;
                }

                // Success path: stream the response body directly to the
                // client without buffering. This is critical for SSE/streaming
                // responses from LLM APIs.
                state.pool.record(idx, true, Some(status.as_u16()), None);
                let entry = RequestLogEntry {
                    id: request_id,
                    ts: chrono::Utc::now().timestamp(),
                    method: parts.method.to_string(),
                    path: parts
                        .uri
                        .path_and_query()
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "/".into()),
                    model: model.clone(),
                    status: status.as_u16(),
                    key_label: label,
                    key_id: key.id.clone(),
                    latency_ms: start.elapsed().as_millis() as u64,
                    retries,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    usage_recorded: false,
                    upstream_error: None,
                };
                state.pool.push_log(entry);
                let mut streaming = build_streaming_response(
                    status,
                    resp_headers,
                    resp,
                    state.pool.clone(),
                    request_id,
                    idx,
                );
                inject_cors(&mut streaming, &parts.headers);
                return streaming;
            }
            Err(e) => {
                state
                    .pool
                    .record(idx, false, None, Some(format!("网络错误: {e}")));
                last_err = Some(format!("网络错误: {e}"));
                retries += 1;
                continue;
            }
        }
    }

    let final_status = last_status.unwrap_or(StatusCode::BAD_GATEWAY);
    let entry = RequestLogEntry {
        id: request_id,
        ts: chrono::Utc::now().timestamp(),
        method: parts.method.to_string(),
        path: parts
            .uri
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "/".into()),
        model,
        status: final_status.as_u16(),
        key_label: last_label,
        key_id: last_id,
        latency_ms: start.elapsed().as_millis() as u64,
        retries,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        usage_recorded: false,
        upstream_error: last_err,
    };
    state.pool.push_log(entry);

    if let (Some(status), Some(bytes)) = (last_status, last_body) {
        let mut resp = build_response(status, last_headers.unwrap_or_default(), bytes);
        inject_cors(&mut resp, &parts.headers);
        return resp;
    }
    let mut resp = error_response(
        StatusCode::BAD_GATEWAY,
        "所有 API Key 都请求失败。请检查密钥与上游网络。".into(),
    );
    inject_cors(&mut resp, &parts.headers);
    resp
}

fn should_retry(status: StatusCode) -> bool {
    matches!(
        status.as_u16(),
        401 | 402 | 403 | 407 | 408 | 425 | 429 | 500 | 502 | 503 | 504
    )
}

fn reqwest_method(m: &Method) -> reqwest::Method {
    reqwest::Method::from_bytes(m.as_str().as_bytes()).unwrap_or(reqwest::Method::GET)
}

fn preview(bytes: &bytes::Bytes) -> String {
    String::from_utf8_lossy(bytes).chars().take(160).collect()
}

fn normalize_proxy_path(upstream: &str, path_and_query: &str) -> String {
    let path_and_query = if path_and_query.starts_with('/') {
        path_and_query.to_string()
    } else {
        format!("/{path_and_query}")
    };
    let upstream_has_v1 = upstream.trim_end_matches('/').ends_with("/v1");
    let (path, query) = path_and_query
        .split_once('?')
        .map(|(path, query)| (path, Some(query)))
        .unwrap_or((path_and_query.as_str(), None));

    let normalized_path = if upstream_has_v1 {
        path.strip_prefix("/v1").filter(|p| p.starts_with('/')).unwrap_or(path)
    } else if path.starts_with("/v1/") || path == "/v1" {
        path
    } else {
        let first = path.trim_start_matches('/').split('/').next().unwrap_or_default();
        match first {
            "models" | "responses" | "chat" | "completions" | "messages" | "usage"
            | "embeddings" | "images" | "audio" | "moderations" => {
                return match query {
                    Some(query) => format!("/v1{path}?{query}"),
                    None => format!("/v1{path}"),
                };
            }
            _ => path,
        }
    };

    match query {
        Some(query) => format!("{normalized_path}?{query}"),
        None => normalized_path.to_string(),
    }
}

fn build_upstream_url(upstream: &str, incoming: &Uri) -> Result<String> {
    let path_and_query = incoming.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let path = normalize_proxy_path(upstream, path_and_query);
    Ok(format!("{}{}", upstream.trim_end_matches('/'), path))
}

fn build_response(status: StatusCode, headers: HeaderMap, body: bytes::Bytes) -> Response {
    let mut filtered = headers.clone();
    strip_hop_headers(&mut filtered);
    let mut builder = Response::builder().status(status);
    if let Some(h) = builder.headers_mut() {
        *h = filtered;
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "构造响应失败".into()))
}

/// If the request is a streaming chat/completions call, inject
/// `stream_options: { include_usage: true }` so the upstream returns token
/// usage in the final SSE chunk. This is transparent to the CLI client.
/// Note: The Responses API (/v1/responses) always returns usage in
/// response.completed events, so no injection is needed there.
fn maybe_inject_stream_options(
    body: &bytes::Bytes,
    parts: &axum::http::request::Parts,
) -> bytes::Bytes {
    // Only modify POST requests to chat/completions-like endpoints
    if parts.method != Method::POST {
        return body.clone();
    }
    let path = parts.uri.path();
    // Skip Responses API — it always includes usage in response.completed
    if path.contains("/responses") {
        return body.clone();
    }
    if !path.contains("/chat/completions") && !path.contains("/completions") {
        return body.clone();
    }
    // Try to parse as JSON
    let mut json: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return body.clone(),
    };
    // Only inject if stream is true
    let is_stream = json
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    if !is_stream {
        return body.clone();
    }
    // Inject stream_options.include_usage = true
    let Some(root) = json.as_object_mut() else {
        return body.clone();
    };
    let stream_options = root
        .entry("stream_options")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(obj) = stream_options.as_object_mut() {
        obj.insert("include_usage".to_string(), serde_json::Value::Bool(true));
    }
    match serde_json::to_vec(&json) {
        Ok(new_body) => bytes::Bytes::from(new_body),
        Err(_) => body.clone(),
    }
}

/// Stream the upstream response body directly to the client without buffering.
/// This enables real-time SSE streaming for LLM chat completions.
/// Also extracts token usage from SSE chunks when available.
fn build_streaming_response(
    status: StatusCode,
    headers: HeaderMap,
    resp: reqwest::Response,
    pool: Arc<Pool>,
    request_id: u64,
    key_idx: usize,
) -> Response {
    use futures_util::StreamExt;

    let mut filtered = headers.clone();
    strip_hop_headers(&mut filtered);

    // Buffer for incomplete lines that span across chunks
    let partial_buf = Arc::new(parking_lot::Mutex::new(String::new()));

    let byte_stream = resp.bytes_stream().map(move |chunk| {
        match &chunk {
            Ok(bytes) => {
                if let Ok(text) = std::str::from_utf8(bytes) {
                    let mut buf = partial_buf.lock();
                    buf.push_str(text);

                    // Process all complete lines
                    while let Some(newline_pos) = buf.find('\n') {
                        let line: String = buf.drain(..=newline_pos).collect();
                        extract_tokens_from_line(line.trim(), &pool, request_id, key_idx);
                    }

                    // If the remaining buffer looks like a complete SSE data
                    // line without a trailing newline (end of stream), try it
                    if buf.contains("\"usage\"") && buf.contains("\"total_tokens\"") {
                        let snapshot = buf.clone();
                        extract_tokens_from_line(snapshot.trim(), &pool, request_id, key_idx);
                    }
                }
            }
            Err(_) => {}
        }
        chunk
            .map(|b| axum::body::Bytes::from(b.to_vec()))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e.to_string()))
    });
    let body = Body::from_stream(byte_stream);

    let mut builder = Response::builder().status(status);
    if let Some(h) = builder.headers_mut() {
        *h = filtered;
    }
    builder
        .body(body)
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "构造流式响应失败".into()))
}

/// Parse a single line for token usage data and record it.
fn extract_tokens_from_line(line: &str, pool: &Pool, request_id: u64, key_idx: usize) {
    let data = if let Some(d) = line.strip_prefix("data: ") {
        d.trim()
    } else if line.starts_with('{') {
        line
    } else {
        return;
    };
    if data == "[DONE]" || data.is_empty() {
        return;
    }
    // Try to parse as JSON
    let v: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Strategy 1: Direct "usage" field (OpenAI Chat Completions API)
    if let Some(usage) = v.get("usage") {
        if usage.is_object() && !usage.is_null() {
            record_usage_from_value(usage, pool, request_id, key_idx);
            return;
        }
    }

    // Strategy 2: Responses API — "response.completed" event
    // Format: {"type":"response.completed","response":{"usage":{...}}}
    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if event_type == "response.completed" || event_type == "response.done" {
        if let Some(resp) = v.get("response") {
            if let Some(usage) = resp.get("usage") {
                record_usage_from_value(usage, pool, request_id, key_idx);
                return;
            }
        }
    }

    // Strategy 3: Anthropic message_delta event with usage
    if event_type == "message_delta" {
        if let Some(usage) = v.get("usage") {
            record_usage_from_value(usage, pool, request_id, key_idx);
            return;
        }
    }

    // Strategy 4: Anthropic message_start event with usage in message
    if event_type == "message_start" {
        if let Some(msg) = v.get("message") {
            if let Some(usage) = msg.get("usage") {
                record_usage_from_value(usage, pool, request_id, key_idx);
                return;
            }
        }
    }

    // Strategy 5: x_usage or _usage variants
    if let Some(usage) = v.get("x_usage").or_else(|| v.get("_usage")) {
        if usage.is_object() {
            record_usage_from_value(usage, pool, request_id, key_idx);
        }
    }
}

fn record_usage_from_value(usage: &serde_json::Value, pool: &Pool, request_id: u64, key_idx: usize) {
    let input = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);
    let output = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    // Cache read: multiple possible locations
    let cache_read = usage
        .get("cache_read_input_tokens")
        .or_else(|| usage.get("cached_tokens"))
        .or_else(|| usage.get("prompt_cache_hit_tokens"))
        .and_then(|t| t.as_u64())
        // OpenAI Responses API: input_tokens_details.cached_tokens
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|t| t.as_u64())
        })
        // OpenAI Chat Completions: prompt_tokens_details.cached_tokens
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|t| t.as_u64())
        })
        .unwrap_or(0);

    let cache_write = usage
        .get("cache_creation_input_tokens")
        .or_else(|| usage.get("prompt_cache_miss_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    let total = usage
        .get("total_tokens")
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    // Use total_tokens if available and no breakdown, otherwise use breakdown
    let effective_input = if input > 0 {
        input
    } else if total > 0 {
        total / 2
    } else {
        0
    };
    let effective_output = if output > 0 {
        output
    } else if total > 0 {
        total - effective_input
    } else {
        0
    };

    if effective_input > 0 || effective_output > 0 || cache_read > 0 || cache_write > 0 {
        if pool.record_request_usage(
            request_id,
            effective_input,
            effective_output,
            cache_read,
            cache_write,
        ) {
            pool.add_usage(effective_input, effective_output, cache_read, cache_write);
            pool.add_key_usage(key_idx, effective_input, effective_output, cache_read, cache_write);
        }
    }
}

fn error_response(status: StatusCode, msg: String) -> Response {
    let body = serde_json::json!({
        "error": {
            "type": "beeapi_switch_proxy",
            "message": msg,
        }
    });
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    (status, [("content-type", "application/json")], bytes).into_response()
}

// --------------------------------------------------------------------------
// LAN IP discovery
// --------------------------------------------------------------------------

/// Best-effort pick of a routable LAN IPv4 address. We open a UDP socket to
/// a public address, read the interface the OS picked, and never actually
/// send anything. Falls back to 127.0.0.1 if everything fails.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(_) => None,
    }
}

fn extract_model_from_body(body: &bytes::Bytes) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
