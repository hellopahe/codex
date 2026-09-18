use super::*;
use codex_http_client::network_monitor::Phase;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::time::Duration;

fn state(now: Instant) -> Snapshot {
    Snapshot {
        transport: "WebSocket",
        endpoint: "wss://chatgpt.com".into(),
        phase: Phase::Receiving,
        started: now - Duration::from_secs(12),
        phase_since: now - Duration::from_secs(8),
        finished: None,
        last_received: Some(now - Duration::from_secs(2)),
        sent_bytes: 1024,
        received_bytes: 2048,
        received_chunks: 12,
        status: Some(101),
        dns: "已解析 2 个地址".into(),
        tcp: "127.0.0.1:443".into(),
        tls: "TLSv1_3".into(),
        connection: "复用 WebSocket".into(),
        send_confirmed: true,
        failure_at: None,
        content: Some(("推理片段", now - Duration::from_secs(3))),
    }
}

#[test]
fn network_monitor_snapshot_and_narrow_terminal() {
    let now = Instant::now();
    let state = state(now);
    for width in [100, 1] {
        let mut terminal = Terminal::new(TestBackend::new(width, 3)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new(lines(Some(&state), now)), frame.area())
            })
            .unwrap();
        insta::assert_snapshot!(format!("network_monitor_width_{width}"), terminal.backend());
    }
}

#[test]
fn finished_request_stops_the_silence_clock() {
    let now = Instant::now();
    let mut state = state(now);
    state.phase = Phase::Complete;
    state.finished = Some(now);
    assert_eq!(
        lines(Some(&state), now),
        lines(Some(&state), now + Duration::from_secs(300))
    );
}
