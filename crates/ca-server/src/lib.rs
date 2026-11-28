//! The IDE's server: the page built into the binary and one live stream (a WebSocket) between the page and the session,
//! for this machine only. It listens on 127.0.0.1. A key made for the run is in the address it prints; the first visit
//! with it gets an HttpOnly, SameSite=Strict cookie, and every request after needs that cookie. The Host must be this
//! server's, a page of another site cannot open the stream (its Origin) nor have the page fetched (Sec-Fetch-Site), and
//! no page may frame it.
#![forbid(unsafe_code)]

pub mod relay;

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::Full;
use hyper::header::{self, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

mod built {
    include!(concat!(env!("OUT_DIR"), "/page.rs"));
}

/// The page built into this binary (`web/dist`); empty when it was built without one.
pub fn built_page() -> &'static [(&'static str, &'static [u8])] {
    built::FILES
}

/// What the page sends is at most this big (a message typed, an answer).
const MOST_MESSAGE: usize = 1 << 20;

/// The cookie's name.
const COOKIE: &str = "code_arena";

/// A page's message, and which page sent it (a number per connection).
pub type FromPage = (u64, Value);

/// The server, listening.
pub struct Started {
    /// The address to open: with the run's key, which the first visit turns into a cookie.
    pub url: String,
    pub port: u16,
    /// What the pages send; `{"type": "hello"}` when one connects, `{"type": "bye"}` when it leaves.
    pub from_page: mpsc::UnboundedReceiver<FromPage>,
    /// What every page connected is sent (JSON text).
    pub to_pages: broadcast::Sender<String>,
    /// The way in for the relay ([`relay::run`]): its pages join the session as these do.
    pub local: relay::Local,
}

struct Shared {
    key: String,
    port: u16,
    page: &'static [(&'static str, &'static [u8])],
    from_page: mpsc::UnboundedSender<FromPage>,
    to_pages: broadcast::Sender<String>,
    next: AtomicU64,
}

fn new_key() -> std::io::Result<String> {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Listens on 127.0.0.1 (`port` 0: any free one) and serves `page` until the process ends.
pub async fn start(port: u16, page: &'static [(&'static str, &'static [u8])]) -> std::io::Result<Started> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    let port = listener.local_addr()?.port();
    let key = new_key()?;
    let (tx, rx) = mpsc::unbounded_channel();
    let (out, _) = broadcast::channel(4096);
    let shared = Arc::new(Shared {
        key: key.clone(),
        port,
        page,
        from_page: tx,
        to_pages: out.clone(),
        next: AtomicU64::new(1),
    });
    let local = relay::Local(shared.clone());
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let shared = shared.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |req| {
                    let shared = shared.clone();
                    async move { Ok::<_, Infallible>(handle(req, &shared)) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
    Ok(Started { url: format!("http://127.0.0.1:{port}/?k={key}"), port, from_page: rx, to_pages: out, local })
}

fn same(a: &str, b: &str) -> bool {
    // Compared whole, whatever differs first: no early way out to time.
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn header(req: &Request<hyper::body::Incoming>, name: header::HeaderName) -> Option<&str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

fn plain(status: StatusCode, text: &str) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(text.to_string())));
    *r.status_mut() = status;
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    r
}

fn kind_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn guarded(mut r: Response<Full<Bytes>>, port: u16) -> Response<Full<Bytes>> {
    let h = r.headers_mut();
    let csp = format!(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; worker-src 'self' blob:; connect-src 'self' ws://127.0.0.1:{port} ws://localhost:{port}; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
    );
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    r
}

fn handle(req: Request<hyper::body::Incoming>, s: &Arc<Shared>) -> Response<Full<Bytes>> {
    guarded(route(req, s), s.port)
}

fn route(req: Request<hyper::body::Incoming>, s: &Arc<Shared>) -> Response<Full<Bytes>> {
    // This server's own name only: a name of another site pointed here (DNS rebinding) is refused.
    let hosts = [format!("127.0.0.1:{}", s.port), format!("localhost:{}", s.port)];
    if !header(&req, header::HOST).is_some_and(|h| hosts.iter().any(|x| x == h)) {
        return plain(
            StatusCode::MISDIRECTED_REQUEST,
            "This is Code Arena's own address: open it as code-arena ide printed it.",
        );
    }
    // Asked for by a page of another site: refused.
    if let Some(site) = header(&req, header::HeaderName::from_static("sec-fetch-site"))
        && !matches!(site, "same-origin" | "none")
    {
        return plain(StatusCode::FORBIDDEN, "Only Code Arena's own page may ask this.");
    }
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("");
    // The first visit, with the run's key: the cookie, and the address without the key.
    if path == "/"
        && let Some(k) = query.split('&').find_map(|kv| kv.strip_prefix("k="))
    {
        if !same(k, &s.key) {
            return plain(
                StatusCode::UNAUTHORIZED,
                "That key is not this run's: open the address code-arena ide printed.",
            );
        }
        let mut r = Response::new(Full::new(Bytes::new()));
        *r.status_mut() = StatusCode::SEE_OTHER;
        r.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/"));
        if let Ok(v) = HeaderValue::from_str(&format!("{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/", s.key)) {
            r.headers_mut().insert(header::SET_COOKIE, v);
        }
        return r;
    }
    let has_cookie = header(&req, header::COOKIE).is_some_and(|c| {
        c.split(';').filter_map(|kv| kv.trim().strip_prefix(&format!("{COOKIE}="))).any(|v| same(v, &s.key))
    });
    if !has_cookie {
        return plain(
            StatusCode::UNAUTHORIZED,
            "Open the address code-arena ide printed in the terminal: it holds this run's key.",
        );
    }
    if path == "/ws" {
        return upgrade(req, s);
    }
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "The page talks to Code Arena over its live stream.");
    }
    let name = if path == "/" { "index.html" } else { path.trim_start_matches('/') };
    match s.page.iter().find(|(p, _)| *p == name) {
        Some((p, bytes)) => {
            let mut r = Response::new(Full::new(Bytes::from_static(bytes)));
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(kind_of(p)));
            // The built files' names change with their content; the page itself is asked again each time.
            let cache = if name == "index.html" { "no-store" } else { "max-age=31536000, immutable" };
            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            r
        }
        None if s.page.is_empty() && name == "index.html" => {
            let mut r = plain(
                StatusCode::OK,
                "This Code Arena was built without its page: build web/ (npm run build) and Code Arena again.",
            );
            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }
        None => plain(StatusCode::NOT_FOUND, "Not here."),
    }
}

fn upgrade(mut req: Request<hyper::body::Incoming>, s: &Arc<Shared>) -> Response<Full<Bytes>> {
    // Only this server's page: a page of another site sends its own Origin.
    let origins = [format!("http://127.0.0.1:{}", s.port), format!("http://localhost:{}", s.port)];
    if !header(&req, header::ORIGIN).is_some_and(|o| origins.iter().any(|x| x == o)) {
        return plain(StatusCode::FORBIDDEN, "Only Code Arena's own page may open its stream.");
    }
    let is_ws = header(&req, header::UPGRADE).is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    let Some(key) = header(&req, header::SEC_WEBSOCKET_KEY).map(str::to_string) else {
        return plain(StatusCode::BAD_REQUEST, "A WebSocket is asked here.");
    };
    if !is_ws {
        return plain(StatusCode::BAD_REQUEST, "A WebSocket is asked here.");
    }
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    let shared = s.clone();
    let on = hyper::upgrade::on(&mut req);
    tokio::spawn(async move {
        if let Ok(upgraded) = on.await {
            let config = WebSocketConfig::default().max_message_size(Some(MOST_MESSAGE));
            let ws = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, Some(config)).await;
            serve(ws, shared).await;
        }
    });
    let mut r = Response::new(Full::new(Bytes::new()));
    *r.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let h = r.headers_mut();
    h.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    h.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    if let Ok(v) = HeaderValue::from_str(&accept) {
        h.insert(header::SEC_WEBSOCKET_ACCEPT, v);
    }
    r
}

async fn serve<S>(ws: WebSocketStream<S>, s: Arc<Shared>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let id = s.next.fetch_add(1, Ordering::SeqCst);
    let (mut sink, mut stream) = ws.split();
    let mut out = s.to_pages.subscribe();
    let _ = s.from_page.send((id, json!({"type": "hello"})));
    let writer = tokio::spawn(async move {
        loop {
            match out.recv().await {
                Ok(text) => {
                    if sink.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
                // Too far behind: the page asks for the whole state again.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if sink.send(Message::text(json!({"type": "behind"}).to_string())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    while let Some(Ok(m)) = stream.next().await {
        match m {
            Message::Text(t) => {
                if let Ok(v) = serde_json::from_str::<Value>(t.as_str())
                    && v.get("type").and_then(Value::as_str).is_some()
                {
                    let _ = s.from_page.send((id, v));
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    writer.abort();
    let _ = s.from_page.send((id, json!({"type": "bye"})));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static PAGE: &[(&str, &[u8])] =
        &[("index.html", b"<!doctype html><title>Code Arena</title>"), ("assets/a.js", b"1")];

    async fn ask(port: u16, text: &str) -> String {
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
        c.write_all(text.as_bytes()).await.expect("write");
        let mut out = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), c.read_to_end(&mut out)).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    fn get(port: u16, path: &str, extra: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n{extra}\r\n")
    }

    #[tokio::test]
    async fn the_key_becomes_a_cookie_and_nothing_is_served_without_it() {
        let s = start(0, PAGE).await.expect("start");
        let port = s.port;
        let key = s.url.split("k=").nth(1).expect("key").to_string();
        assert_eq!(key.len(), 48);
        // No cookie: refused.
        let r = ask(port, &get(port, "/", "")).await;
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
        // A wrong key: refused.
        let r = ask(port, &get(port, "/?k=00", "")).await;
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
        // The run's key: the cookie, and the address without it.
        let r = ask(port, &get(port, &format!("/?k={key}"), "")).await;
        assert!(r.starts_with("HTTP/1.1 303"), "{r}");
        assert!(r.contains(&format!("set-cookie: code_arena={key}; HttpOnly; SameSite=Strict; Path=/")), "{r}");
        let cookie = format!("Cookie: code_arena={key}\r\n");
        let r = ask(port, &get(port, "/", &cookie)).await;
        assert!(r.starts_with("HTTP/1.1 200") && r.contains("<title>Code Arena</title>"), "{r}");
        assert!(r.contains("x-frame-options: DENY") && r.contains("frame-ancestors 'none'"), "{r}");
        assert!(r.contains("cache-control: no-store"), "{r}");
        let r = ask(port, &get(port, "/assets/a.js", &cookie)).await;
        assert!(r.contains("content-type: text/javascript"), "{r}");
        // Another site's name for this address, another site's page, a body sent: refused.
        let r = ask(port, &format!("GET / HTTP/1.1\r\nHost: evil.example:{port}\r\nConnection: close\r\n{cookie}\r\n"))
            .await;
        assert!(r.starts_with("HTTP/1.1 421"), "{r}");
        let r = ask(port, &get(port, "/", &format!("{cookie}Sec-Fetch-Site: cross-site\r\n"))).await;
        assert!(r.starts_with("HTTP/1.1 403"), "{r}");
        let r = ask(
            port,
            &format!(
                "POST /x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: 0\r\n{cookie}\r\n"
            ),
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 405"), "{r}");
    }

    #[tokio::test]
    async fn the_stream_carries_both_ways_for_this_page_only() {
        let mut s = start(0, PAGE).await.expect("start");
        let port = s.port;
        let key = s.url.split("k=").nth(1).expect("key").to_string();
        let url = format!("ws://127.0.0.1:{port}/ws");
        let ask_with = |origin: &str| {
            let mut r = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url.as_str())
                .expect("request");
            r.headers_mut().insert("cookie", HeaderValue::from_str(&format!("code_arena={key}")).expect("h"));
            r.headers_mut().insert("origin", HeaderValue::from_str(origin).expect("h"));
            r
        };
        // Another site's page: refused.
        assert!(tokio_tungstenite::connect_async(ask_with("https://evil.example")).await.is_err());
        let (mut ws, _) =
            tokio_tungstenite::connect_async(ask_with(&format!("http://127.0.0.1:{port}"))).await.expect("ws");
        let (id, hello) = s.from_page.recv().await.expect("hello");
        assert_eq!(hello["type"], "hello");
        ws.send(Message::text(r#"{"type": "send", "text": "hi"}"#)).await.expect("send");
        let (from, v) = s.from_page.recv().await.expect("message");
        assert_eq!((from, v["text"].as_str()), (id, Some("hi")));
        s.to_pages.send(r#"{"type":"text","delta":"Hello"}"#.into()).expect("broadcast");
        let got = ws.next().await.expect("one").expect("ok");
        assert_eq!(got.into_text().expect("text").as_str(), r#"{"type":"text","delta":"Hello"}"#);
        ws.close(None).await.expect("close");
        let (_, bye) = s.from_page.recv().await.expect("bye");
        assert_eq!(bye["type"], "bye");
    }
}
