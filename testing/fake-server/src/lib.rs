//! A local HTTP server for the SDK's tests: each request it receives is
//! answered by a handler the test gives it.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A request as the server received it.
#[derive(Clone, Debug)]
pub struct Received {
    pub method: String,
    /// The path as it was sent, escaped.
    pub path: String,
    /// The query as it was sent, escaped; empty without one.
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl Received {
    /// The first value of the header `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The query parameters, decoded, in the order they were sent.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        self.query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (decode(k), decode(v))
            })
            .collect()
    }

    /// The values of the query parameter `name`.
    pub fn query_values(&self, name: &str) -> Vec<String> {
        self.query_pairs()
            .into_iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v)
            .collect()
    }

    /// The body as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Decodes a form-encoded query component.
fn decode(s: &str) -> String {
    let bytes = s.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the server answers with.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// A 200 with a JSON body.
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
            body: body.into().into_bytes(),
        }
    }

    /// The response with another status.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// The response with one more header.
    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }
}

/// A running server; it stops when dropped.
pub struct FakeServer {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl FakeServer {
    /// Starts a server answering every request with `handler`.
    pub async fn start<F, Fut>(handler: F) -> Self
    where
        F: Fn(Received) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a local port");
        let addr = listener.local_addr().expect("the bound address");
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    let service = service_fn(move |request: hyper::Request<Incoming>| {
                        let handler = Arc::clone(&handler);
                        async move {
                            let (parts, body) = request.into_parts();
                            let body = body
                                .collect()
                                .await
                                .map(|b| b.to_bytes())
                                .unwrap_or_default();
                            let received = Received {
                                method: parts.method.to_string(),
                                path: parts.uri.path().to_owned(),
                                query: parts.uri.query().unwrap_or_default().to_owned(),
                                headers: parts
                                    .headers
                                    .iter()
                                    .map(|(n, v)| {
                                        (n.to_string(), v.to_str().unwrap_or_default().to_owned())
                                    })
                                    .collect(),
                                body,
                            };
                            let answer = handler(received).await;
                            let mut response = hyper::Response::builder().status(answer.status);
                            for (name, value) in &answer.headers {
                                response = response.header(name, value);
                            }
                            Ok::<_, Infallible>(
                                response
                                    .body(Full::new(Bytes::from(answer.body)))
                                    .expect("a valid response"),
                            )
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self { addr, task }
    }

    /// The server's base URL, such as `http://127.0.0.1:41234`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
