use super::*;
use pretty_assertions::assert_eq;

#[test]
fn late_events_cannot_overwrite_a_new_attempt_or_another_thread() {
    let old = Probe::begin("isolation-a", "https://example.com", "HTTP");
    let new = Probe::begin("isolation-a", "https://example.com", "WebSocket");
    let other = Probe::begin("isolation-b", "https://example.com", "HTTP");
    new.phase(Phase::Waiting);
    other.phase(Phase::Tcp);
    old.received(100);
    old.finish(Phase::Failed);
    let a = snapshot("isolation-a").unwrap();
    let b = snapshot("isolation-b").unwrap();
    assert_eq!(
        (a.phase, a.received_bytes, b.phase),
        (Phase::Waiting, 0, Phase::Tcp)
    );
}

#[test]
fn dropping_a_request_settles_it_and_terminal_events_are_sticky() {
    let cancelled = Probe::begin("cancel-test", "https://example.com", "HTTP");
    {
        let _guard = cancelled.guard();
        cancelled.phase(Phase::Waiting);
    }
    cancelled.received(10);
    assert_eq!(snapshot("cancel-test").unwrap().phase, Phase::Cancelled);
    let done = Probe::begin("complete-test", "https://example.com", "HTTP");
    {
        let _guard = done.guard();
        done.finish(Phase::Complete);
    }
    assert_eq!(snapshot("complete-test").unwrap().phase, Phase::Complete);
}

#[test]
fn socket_reuse_resets_exchange_counters_and_retains_connection_facts() {
    let connected = Probe::begin("reuse-test", "wss://example.com", "WebSocket");
    connected.update(|s| {
        s.tls = "TLS 1.3".into();
        s.received_bytes = 1000;
    });
    connected.finish(Phase::Complete);
    let next = connected.exchange("reuse-test", "复用 WebSocket");
    next.received(40);
    let s = snapshot("reuse-test").unwrap();
    assert_eq!(
        (s.received_bytes, s.received_chunks, s.tls, s.finished),
        (40, 1, "TLS 1.3".into(), None)
    );
}

#[test]
fn display_never_retains_url_credentials_paths_or_queries() {
    assert_eq!(
        endpoint_label("https://name:secret@example.com:444/secret-key?token=password#secret"),
        "https://example.com:444"
    );
}

#[test]
fn fast_response_does_not_regress_to_waiting_when_send_completes() {
    let probe = Probe::begin("fast-response-test", "wss://example.com", "WebSocket");
    probe.phase(Phase::Sending);
    probe.received(30);
    probe.local_send_complete();
    let s = snapshot("fast-response-test").unwrap();
    assert_eq!(
        (s.phase, s.send_confirmed, s.received_bytes),
        (Phase::Receiving, true, 30)
    );
}

#[tokio::test]
async fn body_observer_counts_bytes_and_settles_on_eof() {
    use crate::transport::HttpTransport;
    use futures::StreamExt;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(&mut socket);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).await.unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
        }
        assert!(headers.contains("thread-id: http-body-test"));
        assert!(!headers.contains("network-monitor"));
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
            .await
            .unwrap();
    });
    let mut request =
        crate::Request::new(http::Method::POST, format!("http://{address}/responses"));
    request.headers.insert(
        "thread-id",
        http::HeaderValue::from_static("http-body-test"),
    );
    let transport =
        crate::ReqwestTransport::new(reqwest::Client::builder().no_proxy().build().unwrap());
    let mut stream = transport.stream(request).await.unwrap().bytes;
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        body.extend(chunk.unwrap());
    }
    server.await.unwrap();
    assert_eq!(body, b"hello");
    // Integration runs opt in through the parent process, never mutate process env in tests.
    if enabled() {
        let s = snapshot("http-body-test").unwrap();
        assert_eq!(
            (s.phase, s.status, s.received_bytes),
            (Phase::Complete, Some(200), 5)
        );
    }
}
