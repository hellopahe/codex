use crate::client::HttpClient;
use crate::client::RequestBuilder;
use crate::error::TransportError;
use crate::network_monitor::ExchangeGuard;
use crate::network_monitor::Phase;
use crate::network_monitor::Probe;
use crate::request::Request;
use crate::request::RequestBody;
use crate::request::Response;
use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;
use futures::stream::BoxStream;
use http::HeaderMap;
use http::Method;
use http::StatusCode;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tracing::Level;
use tracing::enabled;
use tracing::trace;

pub type ByteStream = BoxStream<'static, Result<Bytes, TransportError>>;

pub struct StreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub bytes: ByteStream,
    /// Local observation handle; this metadata never goes on the wire.
    pub monitor: Probe,
}

pub trait HttpTransport: Send + Sync {
    fn execute(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = Result<Response, TransportError>> + Send;
    fn stream(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = Result<StreamResponse, TransportError>> + Send;
}

#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: HttpClient,
}

impl ReqwestTransport {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client: HttpClient::new(client),
        }
    }

    pub fn from_http_client(client: HttpClient) -> Self {
        Self { client }
    }

    fn build(&self, req: Request, probe: &Probe) -> Result<RequestBuilder, TransportError> {
        let prepared = req.prepare_body_for_send().map_err(TransportError::Build)?;
        probe.update(|s| s.sent_bytes = prepared.body.as_ref().map_or(0, bytes::Bytes::len));

        let Request {
            method,
            url,
            headers: _,
            body: _,
            compression: _,
            timeout,
        } = req;

        let mut builder = self.client.request(
            Method::from_bytes(method.as_str().as_bytes()).unwrap_or(Method::GET),
            &url,
        );

        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }

        builder = builder.headers(prepared.headers);
        if let Some(body) = prepared.body {
            builder = builder.body(body);
        }
        Ok(builder)
    }

    fn map_error(err: reqwest::Error) -> TransportError {
        if err.is_connect() {
            TransportError::Connection(err.without_url())
        } else if err.is_timeout() {
            TransportError::Timeout
        } else {
            TransportError::Network(err.to_string())
        }
    }

    fn trace_request(&self, req: &Request) {
        if self.client.request_logging_enabled() && enabled!(Level::TRACE) {
            trace!(
                "{} to {}: {}",
                req.method,
                req.url,
                request_body_for_trace(req)
            );
        }
    }
}

fn request_body_for_trace(req: &Request) -> String {
    match req.body.as_ref() {
        Some(RequestBody::Json(body)) => body.to_string(),
        Some(RequestBody::EncodedJson(body)) => {
            String::from_utf8_lossy(body.trace_bytes()).into_owned()
        }
        Some(RequestBody::Raw(body)) => format!("<raw body: {} bytes>", body.len()),
        None => String::new(),
    }
}

impl HttpTransport for ReqwestTransport {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        self.trace_request(&req);
        let probe = Probe::from_headers(&req.headers, &req.url, "HTTP");
        let _guard = probe.guard();
        let url = req.url.clone();
        let builder = self.build(req, &probe)?;
        probe.phase(Phase::Waiting);
        let resp = builder.send().await.map_err(|e| {
            probe.finish(Phase::Failed);
            Self::map_error(e)
        })?;
        observe_headers(&probe, &resp);
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await.map_err(|e| {
            probe.finish(Phase::Failed);
            Self::map_error(e)
        })?;
        probe.received(bytes.len());
        probe.finish(if status.is_success() {
            Phase::Complete
        } else {
            Phase::Failed
        });
        if !status.is_success() {
            let body = String::from_utf8(bytes.to_vec()).ok();
            return Err(TransportError::Http {
                status,
                url: Some(url),
                headers: Some(headers),
                body,
            });
        }
        Ok(Response {
            status,
            headers,
            body: bytes,
        })
    }

    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
        self.trace_request(&req);
        let probe = Probe::from_headers(&req.headers, &req.url, "HTTP/SSE");
        let guard = probe.guard();
        let url = req.url.clone();
        let builder = self.build(req, &probe)?;
        probe.phase(Phase::Waiting);
        let resp = builder.send().await.map_err(|e| {
            probe.finish(Phase::Failed);
            Self::map_error(e)
        })?;
        observe_headers(&probe, &resp);
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            probe.finish(Phase::Failed);
            let body = resp.text().await.ok();
            return Err(TransportError::Http {
                status,
                url: Some(url),
                headers: Some(headers),
                body,
            });
        }
        let stream = resp
            .bytes_stream()
            .map(|result| result.map_err(Self::map_error));
        Ok(StreamResponse {
            status,
            headers,
            monitor: probe.clone(),
            bytes: Box::pin(MonitoredStream {
                inner: Box::pin(stream),
                probe,
                _guard: guard,
            }),
        })
    }
}

fn observe_headers(probe: &Probe, response: &reqwest::Response) {
    probe.update(|s| {
        s.status = Some(response.status().as_u16());
        s.connection = format!("{:?}", response.version());
        if let Some(peer) = response.remote_addr() {
            s.tcp = format!("已连接 {peer}");
        }
    });
    probe.phase(Phase::Headers);
}

struct MonitoredStream {
    inner: ByteStream,
    probe: Probe,
    _guard: ExchangeGuard,
}

impl Stream for MonitoredStream {
    type Item = Result<Bytes, TransportError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let result = this.inner.as_mut().poll_next(cx);
        match &result {
            Poll::Ready(Some(Ok(bytes))) => this.probe.received(bytes.len()),
            Poll::Ready(Some(Err(_))) => this.probe.finish(Phase::Failed),
            Poll::Ready(None) => this.probe.finish(Phase::Complete),
            Poll::Pending => {}
        }
        result
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
