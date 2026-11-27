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
use axum::response::Response;
use hyper_util::rt::TokioIo;
use hyper_util::server::graceful::Watcher;
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
    /// HTTP/2 (`Some`): the open streams; with none the idle timeout runs.
    /// The HTTP/1 deadlines do not apply.
    h2_streams: Option<usize>,
    /// The request's headers are read and its response is not out yet: past
    /// the read deadline, reads wait instead of failing. Go's background read
    /// only cancels the request's context then, and the response is still
    /// written until the write deadline.
    serving: bool,
    /// The request body is still being read: past the read deadline that
    /// read fails, as Go's ReadTimeout covers the whole request.
    body_pending: bool,
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
                h2_streams: None,
                serving: false,
                body_pending: false,
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
    /// connection deadlines do not apply. As in Go's http2 server, the
    /// connection is closed once it had no open stream for the idle timeout
    /// (`IdleTimeout`, else `ReadTimeout`).
    pub fn http2(&self) {
        let now = Instant::now();
        let mut s = self.state.lock();
        s.h2_streams = Some(0);
        s.read_deadline = self.timeouts.idle().map(|d| now + d);
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
    fn headers_read(&self, body_pending: bool) {
        let now = Instant::now();
        let mut s = self.state.lock();
        if let Some(n) = s.h2_streams.as_mut() {
            *n += 1;
            s.read_deadline = None;
            return;
        }
        s.read_deadline = nonzero(self.timeouts.read).map(|d| s.request_start + d);
        s.write_deadline = nonzero(self.timeouts.write).map(|d| now + d);
        s.response_ended = false;
        s.serving = true;
        s.body_pending = body_pending;
        // A pipelined request read before the last response was flushed.
        s.idle = false;
        s.wake();
    }

    /// The response body ended; it still has to reach the socket.
    fn response_done(&self) {
        let mut s = self.state.lock();
        if let Some(n) = s.h2_streams.as_mut() {
            *n = n.saturating_sub(1);
            if *n == 0 {
                s.read_deadline = self.timeouts.idle().map(|d| Instant::now() + d);
                s.wake();
            }
            return;
        }
        s.response_ended = true;
    }

    /// Flushed: once a finished response is out, the connection idles until
    /// the next request.
    fn flushed(&self) {
        let mut s = self.state.lock();
        if s.response_ended && s.h2_streams.is_none() {
            let now = Instant::now();
            s.response_ended = false;
            s.serving = false;
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
        if s.idle && s.h2_streams.is_none() {
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

    /// Whether a read past the deadline waits for the response to finish;
    /// if so the reader is woken once it has.
    fn read_waits(&self, w: &std::task::Waker) -> bool {
        let mut s = self.state.lock();
        let waits = s.serving && !s.body_pending;
        if waits {
            s.read_waker = Some(w.clone());
        }
        waits
    }

    /// The request body was read to its end or dropped.
    fn body_done(&self) {
        self.state.lock().body_pending = false;
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

impl<T> DeadlineIo<T> {
    fn expired_read(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.deadlines.read_waits(cx.waker()) {
            self.read_sleep = None;
            return Poll::Pending;
        }
        Poll::Ready(Err(timed_out()))
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for DeadlineIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        // Past the deadline reads fail even with data ready, as in Go; while
        // a response is being written they wait for it to end instead.
        if this
            .deadlines
            .read_deadline()
            .is_some_and(|d| Instant::now() >= d)
        {
            return this.expired_read(cx);
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
                    Poll::Ready(()) => this.expired_read(cx),
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

impl<T: AsyncWrite + Unpin> DeadlineIo<T> {
    /// A write under the write deadline: past it nothing more reaches the
    /// socket, as in Go; a blocked write fails when it passes.
    fn write_with(
        &mut self,
        cx: &mut Context<'_>,
        write: impl FnOnce(Pin<&mut T>, &mut Context<'_>) -> Poll<io::Result<usize>>,
    ) -> Poll<io::Result<usize>> {
        if self
            .deadlines
            .write_deadline()
            .is_some_and(|d| Instant::now() >= d)
        {
            return Poll::Ready(Err(timed_out()));
        }
        match write(Pin::new(&mut self.inner), cx) {
            Poll::Pending => match self.check_write(cx) {
                Some(e) => Poll::Ready(Err(e)),
                None => Poll::Pending,
            },
            r => r,
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for DeadlineIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.write_with(cx, |io, cx| io.poll_write(cx, data))
    }

    // Forwarded so hyper queues body chunks as they are instead of copying
    // each one into its own write buffer first.
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.write_with(cx, |io, cx| io.poll_write_vectored(cx, bufs))
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
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

/// One request on the connection, from its headers to its response body's
/// end. Dropped without a response too (an HTTP/2 stream reset while the
/// handler runs), so the connection's stream count always comes back down.
struct InFlight(Deadlines);

impl InFlight {
    fn open(d: &Deadlines, body_pending: bool) -> Self {
        d.headers_read(body_pending);
        InFlight(d.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.response_done();
    }
}

/// A response body that holds `guard` until the body ends or is dropped (the
/// response was written, or the client went away). Size and end of stream
/// are the inner body's, so fixed-size bodies keep their Content-Length.
pub struct GuardedBody<G> {
    inner: Body,
    guard: Option<G>,
}

impl<G: Send + Unpin + 'static> GuardedBody<G> {
    pub fn wrap(inner: Body, guard: G) -> Body {
        Body::new(GuardedBody {
            inner,
            guard: Some(guard),
        })
    }
}

impl<G: Unpin> http_body::Body for GuardedBody<G> {
    type Data = bytes::Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let p = Pin::new(&mut self.inner).poll_frame(cx);
        if matches!(p, Poll::Ready(None)) {
            self.guard = None;
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

/// A request body that tells the connection when it has been read.
struct RequestBody {
    inner: hyper::body::Incoming,
    deadlines: Option<Deadlines>,
}

impl RequestBody {
    fn done(&mut self) {
        if let Some(d) = self.deadlines.take() {
            d.body_done();
        }
    }
}

impl Drop for RequestBody {
    fn drop(&mut self) {
        self.done();
    }
}

impl http_body::Body for RequestBody {
    type Data = bytes::Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let p = Pin::new(&mut self.inner).poll_frame(cx);
        if matches!(p, Poll::Ready(None | Some(Err(_)))) {
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

/// The service hyper runs for one connection: `app` with the connection's
/// deadline hooks (request headers read, response body written).
pub fn service(
    app: Router,
    deadlines: Deadlines,
) -> impl hyper::service::Service<
    http::Request<hyper::body::Incoming>,
    Response = Response,
    Error = std::convert::Infallible,
    Future: Send,
> + Clone
+ Send
+ 'static {
    hyper_util::service::TowerToHyperService::new(tower::service_fn(
        move |r: http::Request<hyper::body::Incoming>| {
            let pending = !http_body::Body::is_end_stream(r.body());
            let request = InFlight::open(&deadlines, pending);
            let r = r.map(|inner| {
                Body::new(RequestBody {
                    inner,
                    deadlines: Some(deadlines.clone()),
                })
            });
            let app = app.clone();
            async move {
                let resp = match tower::ServiceExt::oneshot(app, r).await {
                    Ok(resp) => resp,
                    Err(e) => match e {},
                };
                Ok::<_, std::convert::Infallible>(
                    resp.map(|inner| GuardedBody::wrap(inner, request)),
                )
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

/// Accepts connections until `stop` and runs `serve` on each, then lets
/// open connections finish (the caller bounds the wait).
pub async fn serve_connections<F, Fut>(
    listener: tokio::net::TcpListener,
    stop: CancellationToken,
    serve: F,
) where
    F: Fn(tokio::net::TcpStream, std::net::SocketAddr, Watcher) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let graceful = hyper_util::server::graceful::GracefulShutdown::new();
    loop {
        let (tcp, addr) = tokio::select! {
            () = stop.cancelled() => break,
            c = accept(&listener) => c,
        };
        tokio::spawn(serve(tcp, addr, graceful.watcher()));
    }
    drop(listener);
    graceful.shutdown().await;
}

/// Serves one plain HTTP/1.1 connection with Go's server timeouts.
pub async fn serve_http1(
    tcp: tokio::net::TcpStream,
    app: Router,
    timeouts: Timeouts,
    watcher: Watcher,
) {
    let deadlines = Deadlines::new(timeouts);
    let svc = service(app, deadlines.clone());
    let io = TokioIo::new(DeadlineIo::new(tcp, deadlines));
    let conn = hyper::server::conn::http1::Builder::new().serve_connection(io, svc);
    let _ = watcher.watch(conn).await;
}

/// Serves `app` over plain HTTP/1.1 with Go's server timeouts until `stop`.
pub async fn serve_plain(
    listener: tokio::net::TcpListener,
    app: Router,
    timeouts: Timeouts,
    stop: CancellationToken,
) {
    serve_connections(listener, stop, move |tcp, _, w| {
        serve_http1(tcp, app.clone(), timeouts, w)
    })
    .await;
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
            )
            .route(
                "/stream",
                axum::routing::get(|| async {
                    let parts = futures::StreamExt::then(
                        futures::stream::iter(["a", "b", "c", "end"]),
                        |p| async move {
                            tokio::time::sleep(Duration::from_millis(150)).await;
                            Ok::<_, std::io::Error>(p)
                        },
                    );
                    axum::body::Body::from_stream(parts)
                }),
            );
        let app = app.route(
            "/upload",
            axum::routing::post(
                |body: axum::body::Bytes| async move { format!("got {}", body.len()) },
            ),
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

    /// Go's read timeout only cancels the request's context: a response
    /// still being written goes out in full (here with no write timeout).
    #[tokio::test]
    async fn read_timeout_does_not_cut_responses() {
        let t = Timeouts {
            read: Duration::from_millis(100),
            ..Default::default()
        };
        let (addr, _stop) = server(t).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /stream HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let out = read_all(&mut s).await;
        assert!(out.contains("end"), "{out}");
    }

    #[tokio::test]
    async fn read_timeout_covers_the_request_body() {
        let t = Timeouts {
            read: Duration::from_millis(200),
            ..Default::default()
        };
        let (addr, _stop) = server(t).await;
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"POST /upload HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\nab")
            .await
            .unwrap();
        // The rest of the body would come after the deadline: the request
        // fails instead of the read waiting for a response.
        let out = tokio::time::timeout(Duration::from_secs(3), read_all(&mut s))
            .await
            .expect("connection left open past the read deadline");
        assert!(!out.contains("got 10"), "{out}");
    }

    #[test]
    fn a_cancelled_http2_request_closes_its_stream() {
        let t = Timeouts {
            idle: Duration::from_secs(5),
            ..Default::default()
        };
        let d = Deadlines::new(t);
        d.http2();
        // The handler future is dropped (stream reset) before it responds.
        drop(InFlight::open(&d, false));
        assert_eq!(d.state.lock().h2_streams, Some(0));
        assert!(d.read_deadline().is_some(), "idle timeout must run again");
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
        d.headers_read(false);
        d.response_done();
        // The next request's bytes were already buffered: its headers are
        // read before the previous response is flushed.
        d.flushed();
        d.headers_read(false);
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
