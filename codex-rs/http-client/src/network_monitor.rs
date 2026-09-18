//! Process-local, display-only model transport observations for the custom `codexn` TUI.
//! No payloads, credentials, probes, network requests, or on-disk records are produced.
//! Each attempt owns its own cell: a late event cannot overwrite a newer attempt.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

use http::HeaderMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Dns,
    Tcp,
    ProxyTls,
    ProxyTunnel,
    Tls,
    Upgrade,
    Sending,
    Waiting,
    Headers,
    Receiving,
    Complete,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "获取连接",
            Self::Dns => "DNS 解析",
            Self::Tcp => "TCP 连接",
            Self::ProxyTls => "代理 TLS 握手",
            Self::ProxyTunnel => "建立代理隧道",
            Self::Tls => "TLS 握手",
            Self::Upgrade => "WebSocket 握手",
            Self::Sending => "发送请求",
            Self::Waiting => "等待响应",
            Self::Headers => "已收响应头，等待响应体",
            Self::Receiving => "接收响应",
            Self::Complete => "传输完成",
            Self::Failed => "传输失败",
            Self::Cancelled => "请求已结束／取消",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub transport: &'static str,
    pub endpoint: String,
    pub phase: Phase,
    pub started: Instant,
    pub phase_since: Instant,
    pub finished: Option<Instant>,
    pub last_received: Option<Instant>,
    pub sent_bytes: usize,
    pub received_bytes: usize,
    pub received_chunks: usize,
    pub status: Option<u16>,
    pub dns: String,
    pub tcp: String,
    pub tls: String,
    pub connection: String,
    pub send_confirmed: bool,
    pub failure_at: Option<Phase>,
    pub content: Option<(&'static str, Instant)>,
}

type Cell = Arc<Mutex<Snapshot>>;
static REGISTRY: OnceLock<Mutex<BTreeMap<String, Cell>>> = OnceLock::new();
const MAX_THREADS: usize = 128;

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("CODEX_NETWORK_MONITOR").is_ok_and(|v| v == "1"))
}

pub fn snapshot(thread_id: &str) -> Option<Snapshot> {
    let cell = REGISTRY.get()?.lock().ok()?.get(thread_id)?.clone();
    cell.lock().ok().map(|s| s.clone())
}

/// An optional observation handle. Disabled monitoring is a no-op.
#[derive(Clone, Debug, Default)]
pub struct Probe(Option<Cell>);

impl Probe {
    pub fn active(&self) -> bool {
        self.0.is_some()
    }

    pub fn from_headers(headers: &HeaderMap, endpoint: &str, transport: &'static str) -> Self {
        if !enabled() {
            return Self::default();
        }
        let Some(thread_id) = headers.get("thread-id").and_then(|v| v.to_str().ok()) else {
            return Self::default();
        };
        Self::begin(thread_id, endpoint, transport)
    }

    fn begin(thread_id: &str, endpoint: &str, transport: &'static str) -> Self {
        if thread_id.is_empty() || thread_id.len() > 128 {
            return Self::default();
        }
        let now = Instant::now();
        let cell = Arc::new(Mutex::new(Snapshot {
            transport,
            endpoint: endpoint_label(endpoint),
            phase: Phase::Connecting,
            started: now,
            phase_since: now,
            finished: None,
            last_received: None,
            sent_bytes: 0,
            received_bytes: 0,
            received_chunks: 0,
            status: None,
            dns: "未暴露".into(),
            tcp: "未暴露".into(),
            tls: "未暴露".into(),
            connection: "尚未确认".into(),
            send_confirmed: false,
            failure_at: None,
            content: None,
        }));
        if let Ok(mut registry) = REGISTRY.get_or_init(Mutex::default).lock() {
            if registry.len() >= MAX_THREADS && !registry.contains_key(thread_id) {
                let oldest = registry
                    .iter()
                    .filter_map(|(id, cell)| cell.lock().ok().map(|s| (id.clone(), s.started)))
                    .min_by_key(|(_, started)| *started)
                    .map(|(id, _)| id);
                if let Some(id) = oldest {
                    registry.remove(&id);
                }
            }
            registry.insert(thread_id.to_owned(), cell.clone());
        }
        Self(Some(cell))
    }

    /// Starts a model exchange on an already connected socket, preserving observed connection facts.
    pub fn exchange(&self, thread_id: &str, connection: &'static str) -> Self {
        let Some(prior) = self
            .0
            .as_ref()
            .and_then(|c| c.lock().ok().map(|s| s.clone()))
        else {
            return Self::default();
        };
        let probe = Self::begin(thread_id, &prior.endpoint, prior.transport);
        probe.update(|s| {
            s.dns = if connection == "复用 WebSocket" {
                "复用连接，无新解析".into()
            } else {
                prior.dns
            };
            s.tcp = prior.tcp;
            s.tls = prior.tls;
            s.status = prior.status;
            s.connection = connection.into();
            if connection == "新 WebSocket" {
                s.started = prior.started;
            }
        });
        probe
    }

    pub fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        if let Some(cell) = &self.0
            && let Ok(mut s) = cell.lock()
            && s.finished.is_none()
        {
            f(&mut s);
        }
    }

    pub fn phase(&self, phase: Phase) {
        self.update(|s| {
            if s.phase != phase {
                s.phase = phase;
                s.phase_since = Instant::now();
            }
        });
    }

    pub fn received(&self, bytes: usize) {
        self.phase(Phase::Receiving);
        self.update(|s| {
            s.received_bytes = s.received_bytes.saturating_add(bytes);
            s.received_chunks = s.received_chunks.saturating_add(1);
            s.last_received = Some(Instant::now());
        });
    }

    /// A fast response may arrive before the sending task is rescheduled.
    pub fn local_send_complete(&self) {
        self.update(|s| {
            s.send_confirmed = true;
            if s.received_chunks == 0 {
                s.phase = Phase::Waiting;
                s.phase_since = Instant::now();
            }
        });
    }

    pub fn content(&self, kind: &str) {
        let label = if kind.contains("reasoning") {
            "推理片段"
        } else if kind.contains("output_text") {
            "正文片段"
        } else if kind.contains("tool_call") || kind.contains("function_call") {
            "工具参数"
        } else {
            return;
        };
        self.update(|s| s.content = Some((label, Instant::now())));
    }

    pub fn finish(&self, phase: Phase) {
        if phase == Phase::Failed {
            self.update(|s| s.failure_at = Some(s.phase));
        }
        self.phase(phase);
        self.update(|s| s.finished = Some(Instant::now()));
    }

    pub fn guard(&self) -> ExchangeGuard {
        ExchangeGuard(self.clone())
    }
}

/// Cancellation and dropped streams settle the display without affecting the transport.
pub struct ExchangeGuard(Probe);

impl Drop for ExchangeGuard {
    fn drop(&mut self) {
        self.0.finish(Phase::Cancelled);
    }
}

fn endpoint_label(endpoint: &str) -> String {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return "地址未暴露".into();
    };
    let host = url.host_str().unwrap_or("?");
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    // Path segments can contain credentials too. Host and transport suffice for this display.
    format!("{}://{host}{port}", url.scheme())
}

#[cfg(test)]
#[path = "network_monitor_tests.rs"]
mod tests;
