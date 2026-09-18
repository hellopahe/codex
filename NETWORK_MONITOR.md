# codexn network monitor

This branch is based on Codex CLI 0.155.0. It adds a local network panel above the
original composer, alongside Codex's existing task, tool, approval and compaction
indicators. It does not change model payloads, credentials, version headers,
retry policy, timeouts, proxy selection or certificate verification.

## Install and run

Build and install with `scripts/install-codexn.sh`, then run `codexn` with the
same arguments as `codex`. The executable is installed to
`~/.local/lib/codexn/codex`; the launcher is `~/.local/bin/codexn`.
The official `codex` command is never replaced. Add `~/.local/bin` to your PATH
if it is not already present. `CODEX_NETWORK_MONITOR=0 codexn` hides the monitor.

## What the panel measures

- A separate observation cell per model request and thread. Older attempts,
  other sessions, and subagents cannot overwrite the selected thread's latest
  request. At most 128 thread snapshots remain in memory.
- HTTP response status, response-body bytes/chunks, elapsed time and time since
  the last received chunk. The request body size is its prepared encoded size;
  HTTP send completion is explicitly marked unobserved.
- WebSocket local send completion, message payload bytes/counts, new/reused
  connections, response/control activity and time since the last message.
- Nonempty reasoning/text/tool-argument deltas are timed separately from network
  traffic. Network inactivity never implies that the server is stuck or thinking.
- Default and routed WebSockets expose actual DNS resolution, TCP connection, proxy tunnel,
  and (with Codex's explicit Rustls configuration) TLS/upgrade boundaries. The
  same socket, TLS configuration and handshake parameters are used.
- Completed, failed and cancelled requests stop their clocks. Failure retains
  the observed stage. Codex continues to render its original error details.

## Boundaries

The panel reads the embedded engine's in-process observations. A separately
running remote/daemon app server has no shared observation registry, and shows
that no request has been observed in the current process.

Reqwest HTTP connection internals do not expose every DNS/TCP/TLS boundary.
These fields say `未暴露` instead of guessing or probing. The default WebSocket
path reuses Tungstenite's own environment-proxy parser and tunnel implementation,
including NO_PROXY and SOCKS behavior. A reused WebSocket reports no new DNS resolution.
Byte counters measure HTTP body chunks / WebSocket payloads, not encrypted
packets, framing overhead or all traffic on a shared socket. The panel clips
long lines to the terminal width.

The monitor is display-only: snapshots stay in memory, contain no request or
response content, credentials, URL paths or queries, and are never added to
conversation history or telemetry. Existing Codex telemetry settings still
apply independently.

## Validation

Run the transport suites with monitoring enabled and loopback exempted from
test-machine proxies:

```sh
NO_PROXY=localhost,127.0.0.1,::1 CODEX_NETWORK_MONITOR=1 just test -p codex-http-client -p codex-websocket-client
just test -p codex-api -p codex-tui
```

Tests exercise request isolation, cancellation, connection reuse, URL redaction,
real loopback HTTP byte counting, TLS connections, and terminal snapshots.
