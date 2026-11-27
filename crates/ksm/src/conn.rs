//! Connection deadlines with Go `net/http.Server` semantics for
//! `ReadHeaderTimeout`, `ReadTimeout`, `WriteTimeout` and `IdleTimeout`
//! (`--server-*-timeout`), and the plain HTTP serve loop.
//!
//! Go sets deadlines on the connection: the header deadline starts when a
//! request starts (at accept for the first one, at the first byte after an
//! idle keep-alive wait for later ones), the read deadline covers the whole
//! request from that moment, the write deadline starts once the headers are
//! read, and between requests the connection may idle for `IdleTimeout`
//! (`ReadTimeout` when zero). A zero duration means no deadline. A read or
//! write past its deadline fails, which closes the connection.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};
use tokio_util::sync::CancellationToken;

/// The four server timeouts; zero disables one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timeouts {
    pub read_header: Duration,
    pub read: Duration,
    pub write: Duration,
    pub idle: Duration,
}

fn nonzero(d: Duration) -> Option<Duration> {
    (!d.is_zero()).then_some(d)
}

impl Timeouts {
    /// `ReadHeaderTimeout`, or `ReadTimeout` when zero.
    fn header(&self) -> Option<Duration> {
        nonzero(self.read_header).or(nonzero(self.read))
    }

    /// `IdleTimeout`, or `ReadTimeout` when zero.
    fn idle(&self) -> Option<Duration> {
        nonzero(self.idle).or(nonzero(self.read))
    }

    /// `tlsHandshakeTimeout`: the smallest non-zero of the read header, read
    /// and write timeouts.
    pub fn tls_handshake(&self) -> Option<Duration> {
        [self.read_header, self.read, self.write]
            .into_iter()
            .filter(|d| !d.is_zero())
            .min()
    }
}

#[derive(Debug)]
struct State {
    read_deadline: Option<Instant>,
    write_deadline: Option<Instant>,
    /// Waiting for the next request on a kept-alive connection.
    idle: bool,
    /// When the current request started (its header deadline's origin).
    request_start: Instant,
    /// The response body ended; the connection idles once it is flushed.
    response_ended: bool,
    /// No deadlines apply any more (an HTTP/2 connection).
    off: bool,
    /// Wakers of a pending read and write, woken when deadlines change.
    read_waker: Option<std::task::Waker>,
    write_waker: Option<std::task::Waker>,
}

impl State {
    fn wake(&mut self) {
        if let Some(w) = self.read_waker.take() {
            w.wake();
        }
        if let Some(w) = self.write_waker.take() {
            w.wake();
        }
    }
}

/// Deadline state shared by a connection's IO and its request layer.
#[derive(Clone)]
pub struct Deadlines {
    timeouts: Timeouts,
    state: Arc<Mutex<State>>,
}

impl Deadlines {
    /// A new connection: the first request's header deadline starts now.
    pub fn new(timeouts: Timeouts) -> Self {
        let now = Instant::now();
        Deadlines {
            timeouts,
            state: Arc::new(Mutex::new(State {
                read_deadline: timeouts.header().map(|d| now + d),
                write_deadline: None,
                idle: false,
                request_start: now,
                response_ended: false,
                off: false,
                read_waker: None,
                write_waker: None,
            })),
        }
    }

    /// Sets the deadlines for a TLS handshake (Go's `tlsHandshakeTimeout`)
    /// and returns the deadline.
    pub fn for_handshake(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut s = self.state.lock();
        let d = self.timeouts.tls_handshake().map(|d| now + d);
        s.read_deadline = d;
        s.write_deadline = d;
        s.wake();
        d
    }

    /// HTTP/2 multiplexes requests on one connection, so per-request
    /// connection deadlines do not apply (Go's http2 server handles them per
    /// stream). Only the TLS handshake deadline is used.
    pub fn disable(&self) {
        let mut s = self.state.lock();
        s.off = true;
        s.read_deadline = None;
        s.write_deadline = None;
        s.wake();
    }

    /// The handshake is done: back to the first request's header deadline.
    pub fn handshake_done(&self) {
        let now = Instant::now();
        let mut s = self.state.lock();
        s.read_deadline = self.timeouts.header().map(|d| now + d);
        s.write_deadline = None;
        s.request_start = now;
        s.wake();
    }

    /// Headers read: the read deadline covers the rest of the request, the
    /// write deadline starts.
    fn headers_read(&self) {
        let now = Instant::now();
        let mut s = self.state.lock();
        if s.off {
            return;
        }
        s.read_deadline = nonzero(self.timeouts.read).map(|d| s.request_start + d);
        s.write_deadline = nonzero(self.timeouts.write).map(|d| now + d);
        s.response_ended = false;
        // A pipelined request read before the last response was flushed.
        s.idle = false;
        s.wake();
    }

    /// The response body ended; it still has to reach the socket.
    fn response_done(&self) {
        self.state.lock().response_ended = true;
    }

    /// Flushed: once a finished response is out, the connection idles until
    /// the next request.
    fn flushed(&self) {
        let mut s = self.state.lock();
        if s.response_ended && !s.off {
            let now = Instant::now();
            s.response_ended = false;
            s.idle = true;
            s.read_deadline = self.timeouts.idle().map(|d| now + d);
            s.write_deadline = None;
            s.wake();
        }
    }

    fn park_read(&self, w: &std::task::Waker) {
        self.state.lock().read_waker = Some(w.clone());
    }

    fn park_write(&self, w: &std::task::Waker) {
        self.state.lock().write_waker = Some(w.clone());
    }

    /// Bytes arrived: if the connection was idle, a new request starts now.
    fn bytes_read(&self) {
        let mut s = self.state.lock();
        if s.idle && !s.off {
            let now = Instant::now();
            s.idle = false;
            s.request_start = now;
            s.read_deadline = self.timeouts.header().map(|d| now + d);
            if let Some(w) = s.write_waker.take() {
                w.wake();
            }
        }
    }

    fn read_deadline(&self) -> Option<Instant> {
        self.state.lock().read_deadline
    }

    fn write_deadline(&self) -> Option<Instant> {
        self.state.lock().write_deadline
    }
}

/// A stream that fails reads and writes past the connection's deadlines.
pub struct DeadlineIo<T> {
    inner: T,
    deadlines: Deadlines,
    read_sleep: Option<(Instant, Pin<Box<Sleep>>)>,
    write_sleep: Option<(Instant, Pin<Box<Sleep>>)>,
}

impl<T> DeadlineIo<T> {
    pub fn new(inner: T, deadlines: Deadlines) -> Self {
        DeadlineIo {
            inner,
            deadlines,
            read_sleep: None,
            write_sleep: None,
        }
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "i/o timeout")
}

/// Polls the timer for `deadline`; `Ready` once it has passed.
fn poll_deadline(
    slot: &mut Option<(Instant, Pin<Box<Sleep>>)>,
    deadline: Option<Instant>,
    cx: &mut Context<'_>,
) -> Poll<()> {
    let Some(deadline) = deadline else {
        *slot = None;
        return Poll::Pending;
    };
    if Instant::now() >= deadline {
        return Poll::Ready(());
    }
    match slot {
        Some((at, sleep)) if *at == deadline => sleep.as_mut().poll(cx),
        _ => {
            let mut sleep = Box::pin(tokio::time::sleep_until(deadline));
            let p = sleep.as_mut().poll(cx);
            *slot = Some((deadline, sleep));
            p
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for DeadlineIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        // Past the deadline reads fail even with data ready, as in Go.
        if this
            .deadlines
            .read_deadline()
            .is_some_and(|d| Instant::now() >= d)
        {
            return Poll::Ready(Err(timed_out()));
        }
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    this.deadlines.bytes_read();
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => {
                this.deadlines.park_read(cx.waker());
                let deadline = this.deadlines.read_deadline();
                match poll_deadline(&mut this.read_sleep, deadline, cx) {
                    Poll::Ready(()) => Poll::Ready(Err(timed_out())),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

impl<T: AsyncWrite + Unpin> DeadlineIo<T> {
    fn check_write(&mut self, cx: &mut Context<'_>) -> Option<io::Error> {
        self.deadlines.park_write(cx.waker());
        let deadline = self.deadlines.write_deadline();
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Some(timed_out());
        }
        // Register the timer so a blocked write wakes up at the deadline.
        match poll_deadline(&mut self.write_sleep, deadline, cx) {
            Poll::Ready(()) => Some(timed_out()),
            Poll::Pending => None,
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for DeadlineIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        // Past the deadline nothing more reaches the socket, as in Go.
        if this
            .deadlines
            .write_deadline()
            .is_some_and(|d| Instant::now() >= d)
        {
            return Poll::Ready(Err(timed_out()));
        }
        match Pin::new(&mut this.inner).poll_write(cx, data) {
            Poll::Pending => match this.check_write(cx) {
                Some(e) => Poll::Ready(Err(e)),
                None => Poll::Pending,
            },
            r => r,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_flush(cx) {
            Poll::Pending => match this.check_write(cx) {
                Some(e) => Poll::Ready(Err(e)),
                None => Poll::Pending,
            },
            Poll::Ready(Ok(())) => {
                this.deadlines.flushed();
                Poll::Ready(Ok(()))
            }
            r => r,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A response body that tells the connection when it has been written.
struct NotifyBody {
    inner: Body,
    deadlines: Option<Deadlines>,
}

impl NotifyBody {
    fn done(&mut self) {
        if let Some(d) = self.deadlines.take() {
            d.response_done();
        }
    }
}

impl Drop for NotifyBody {
    fn drop(&mut self) {
        self.done();
    }
}

impl http_body::Body for NotifyBody {
    type Data = bytes::Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let p = Pin::new(&mut self.inner).poll_frame(cx);
        if matches!(p, Poll::Ready(None)) {
            self.done();
        }
        p
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

/// Adds the per-connection deadline hooks to `app`.
pub fn with_deadlines(app: Router, deadlines: Deadlines) -> Router {
    app.layer(axum::middleware::from_fn(
        move |req: Request, next: Next| {
            let d = deadlines.clone();
            async move {
                d.headers_read();
                let resp = next.run(req).await;
                resp.map(|b| {
                    Body::new(NotifyBody {
                        inner: b,
                        deadlines: Some(d),
                    })
                })
            }
        },
    ))
}

/// Accepts the next connection. After an accept error (such as running out
/// of file descriptors) it waits before trying again, 5 ms doubling up to 1 s,
/// as Go's `Server.Serve` does.
pub async fn accept(
    listener: &tokio::net::TcpListener,
) -> (tokio::net::TcpStream, std::net::SocketAddr) {
    let mut delay = Duration::from_millis(5);
    loop {
        match listener.accept().await {
            Ok(c) => return c,
            Err(e) => {
                tracing::error!(error = %e, "accept; retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(1));
            }
        }
    }
}

/// Serves `app` over plain HTTP/1.1 with Go's server timeouts until `stop`,
/// then lets open connections finish (the caller bounds the wait).
pub async fn serve_plain(
    listener: tokio::net::TcpListener,
    app: Router,
    timeouts: Timeouts,
    stop: CancellationToken,
) {
    let graceful = hyper_util::server::graceful::GracefulShutdown::new();
    loop {
        let (tcp, _) = tokio::select! {
            () = stop.cancelled() => break,
            c = accept(&listener) => c,
        };
        let deadlines = Deadlines::new(timeouts);
        let app = with_deadlines(app.clone(), deadlines.clone());
        let watcher = graceful.watcher();
        tokio::spawn(async move {
            let svc = hyper_util::service::TowerToHyperService::new(
                tower::ServiceBuilder::new()
                    .map_request(|r: http::Request<hyper::body::Incoming>| r.map(Body::new))
                    .service(app),
            );
            let io = TokioIo::new(DeadlineIo::new(tcp, deadlines));
            let conn = hyper::server::conn::http1::Builder::new().serve_connection(io, svc);
            let _ = watcher.watch(conn).await;
        });
    }
    drop(listener);
    graceful.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn server(timeouts: Timeouts) -> (std::net::SocketAddr, CancellationToken) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let app = Router::new()
            .route("/", axum::routing::get(|| async { "OK" }))
            .route(
                "/slow",
                axum::routing::get(|| async {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    "late"
                }),
            );
        tokio::spawn(serve_plain(listener, app, timeouts, stop.clone()));
        (addr, stop)
    }

    async fn read_all(s: &mut tokio::net::TcpStream) -> String {
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut out)).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn header_timeout_closes_slow_clients() {
        let t = Timeouts {
            read_header: Duration::from_millis(200),
            ..Default::default()
        };
        let (addr, _stop) = server(t).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n").await.unwrap();
        let start = std::time::Instant::now();
        let out = read_all(&mut s).await;
        assert!(!out.contains("200 OK"), "{out}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn complete_requests_are_served() {
        let t = Timeouts {
            read_header: Duration::from_millis(200),
            ..Default::default()
        };
        let (addr, _stop) = server(t).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert!(read_all(&mut s).await.contains("200 OK"));
    }

    #[tokio::test]
    async fn write_timeout_cuts_slow_responses() {
        let t = Timeouts {
            write: Duration::from_millis(100),
            ..Default::default()
        };
        let (addr, _stop) = server(t).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        assert!(!read_all(&mut s).await.contains("late"));
    }

    #[tokio::test]
    async fn idle_timeout_closes_kept_alive_connections() {
        let t = Timeouts {
            idle: Duration::from_millis(200),
            ..Default::default()
        };
        let (addr, _stop) = server(t).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 1024];
        let n = s.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("200 OK"));
        // No further request: the server closes the connection after the idle timeout.
        let start = std::time::Instant::now();
        let n = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap_or(0);
        assert_eq!(n, 0);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn keep_alive_requests_reset_the_header_deadline() {
        let t = Timeouts {
            read_header: Duration::from_millis(300),
            ..Default::default()
        };
        let (addr, _stop) = server(t).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 1024];
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            let n = s.read(&mut buf).await.unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).contains("200 OK"));
        }
    }

    #[tokio::test]
    async fn pipelined_request_is_not_treated_as_idle() {
        let t = Timeouts {
            read_header: Duration::from_millis(100),
            read: Duration::from_secs(10),
            ..Default::default()
        };
        let d = Deadlines::new(t);
        d.headers_read();
        d.response_done();
        // The next request's bytes were already buffered: its headers are
        // read before the previous response is flushed.
        d.flushed();
        d.headers_read();
        let deadline = d.read_deadline();
        // A later read must not restart the request with the header deadline.
        d.bytes_read();
        assert!(!d.state.lock().idle);
        assert_eq!(d.read_deadline(), deadline);
    }

    #[tokio::test]
    async fn pipelined_requests_are_served() {
        let (addr, _stop) = server(Timeouts::default()).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(read_all(&mut s).await.matches("200 OK").count(), 2);
    }
}
