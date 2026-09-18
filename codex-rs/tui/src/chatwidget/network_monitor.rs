//! Local embedded-engine network display. Observations never become model-visible history.

use codex_http_client::network_monitor::Snapshot;
use codex_protocol::ThreadId;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::time::Instant;

pub(super) fn panel(thread_id: Option<ThreadId>) -> Option<Paragraph<'static>> {
    if !codex_http_client::network_monitor::enabled() {
        return None;
    }
    let snapshot =
        thread_id.and_then(|id| codex_http_client::network_monitor::snapshot(&id.to_string()));
    Some(Paragraph::new(lines(snapshot.as_ref(), Instant::now())))
}

fn lines(snapshot: Option<&Snapshot>, now: Instant) -> Vec<Line<'static>> {
    let content = if let Some(s) = snapshot {
        let end = s.finished.unwrap_or(now);
        let age = end.saturating_duration_since(s.started).as_secs();
        let phase_age = end.saturating_duration_since(s.phase_since).as_secs();
        let failure = s
            .failure_at
            .map(|p| format!("（{}阶段）", p.label()))
            .unwrap_or_default();
        let content = s
            .content
            .map(|(label, at)| {
                format!(
                    " · {label}距今{}秒",
                    end.saturating_duration_since(at).as_secs()
                )
            })
            .unwrap_or_default();
        let title = if s.finished.is_some() {
            "最近网络请求"
        } else {
            "网络"
        };
        let status = s
            .status
            .map(|code| format!(" · HTTP {code}"))
            .unwrap_or_default();
        let silent = s
            .last_received
            .map(|at| {
                format!(
                    " · {}秒无新数据",
                    end.saturating_duration_since(at).as_secs()
                )
            })
            .unwrap_or_else(|| " · 尚无响应体数据".into());
        let sent = if s.send_confirmed {
            "本地已发送"
        } else if s.sent_bytes == 0 {
            "尚未观测请求体"
        } else {
            "已构建／发送未确认"
        };
        vec![
            format!(
                "{title} · {} · {}{failure} · 阶段{phase_age}秒 / 请求{age}秒{status} · {}",
                s.transport,
                s.phase.label(),
                s.connection
            ),
            format!(
                "  ↑{} B（{sent}） ↓{} B / {}块{silent}{content}",
                s.sent_bytes, s.received_bytes, s.received_chunks
            ),
            format!(
                "  DNS：{} · TLS：{} · TCP：{} · {}",
                s.dns, s.tls, s.tcp, s.endpoint
            ),
        ]
    } else {
        vec!["网络 · 当前进程尚未观测到该会话的模型请求".into()]
    };
    content
        .into_iter()
        .map(|text| Line::from(text.italic()).style(crate::style::accent_style()))
        .collect()
}

#[cfg(test)]
#[path = "network_monitor_tests.rs"]
mod tests;
