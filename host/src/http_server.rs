//! Embedded HTTP server that streams live CAN events to a browser.
//!
//! # Usage
//!
//! ```ignore
//! let server = SseServer::start(7878);
//! // Pass server.tx into EventLogger::attach_sse() so every logged event
//! // is also broadcast to all connected browser clients.
//! logger.attach_sse(server.tx.clone());
//! ```
//!
//! # Endpoints
//!
//! | Path       | Description                                              |
//! |------------|----------------------------------------------------------|
//! | `GET /`          | Serves the embedded dashboard HTML page               |
//! | `GET /logo.png`  | Serves the embedded app icon (256 × 256 PNG)          |
//! | `GET /events`    | SSE stream — one JSONL event per `data:` message      |
//!
//! The server binds exclusively to `127.0.0.1` so it is never reachable
//! from outside the local machine.  HTTPS is unnecessary on loopback.

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::http::header;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

// ─── Embedded assets ─────────────────────────────────────────────────────────

/// The dashboard HTML page, compiled into the binary at build time.
const INDEX_HTML: &str = include_str!("../assets/index.html");

/// The app icon PNG, served at `/logo.png` for the browser dashboard.
const LOGO_PNG: &[u8] = include_bytes!("../assets/RustyCAN.iconset/icon_256x256.png");

// ─── Broadcast channel capacity ───────────────────────────────────────────────

/// Number of events buffered per subscriber before oldest are dropped.
///
/// At 500 events/sec a lagged client has ~200 ms to catch up before events
/// are silently dropped (lagged subscribers do not block the sender).
const BROADCAST_CAPACITY: usize = 128;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Handle to the running SSE HTTP server.
///
/// Clone `tx` and pass it to [`EventLogger::attach_sse`] to wire up live
/// streaming.  The server runs for the lifetime of this struct (dropping it
/// does not stop the background thread — it runs until the process exits, which
/// is fine for a desktop app with one lifetime).
pub struct SseServer {
    /// Broadcast sender — clone this to publish events from the logger.
    pub tx: broadcast::Sender<String>,
}

impl SseServer {
    /// Spawn the HTTP server on `127.0.0.1:{port}` in a background thread.
    ///
    /// Returns immediately; the server runs concurrently on a dedicated tokio
    /// runtime so it never interferes with the eframe render thread.
    ///
    /// If the port is already in use the error is printed to stderr and the
    /// server silently does nothing — the rest of the app continues normally.
    pub fn start(port: u16) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let tx_clone = tx.clone();

        std::thread::Builder::new()
            .name("rustycan-http".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .expect("tokio runtime");

                rt.block_on(async move {
                    let addr = SocketAddr::from(([127, 0, 0, 1], port));

                    let app = Router::new()
                        .route("/", get(serve_index))
                        .route("/logo.png", get(serve_logo))
                        .route("/events", get(move || sse_handler(tx_clone.clone())));

                    match tokio::net::TcpListener::bind(addr).await {
                        Ok(listener) => {
                            eprintln!("[rustycan] Live dashboard: http://{addr}/");
                            if let Err(e) = axum::serve(listener, app).await {
                                eprintln!("[rustycan] HTTP server error: {e}");
                            }
                        }
                        Err(e) => {
                            eprintln!("[rustycan] Could not bind http://{addr}/: {e}");
                        }
                    }
                });
            })
            .expect("failed to spawn HTTP server thread");

        SseServer { tx }
    }
}

// ─── Route handlers ───────────────────────────────────────────────────────────

/// Serve the embedded dashboard page with correct `Content-Type`.
async fn serve_index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(INDEX_HTML),
    )
}

/// Serve the embedded app icon with correct `Content-Type`.
async fn serve_logo() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], LOGO_PNG)
}

/// SSE handler — subscribes a new client to the broadcast channel and streams
/// every event as a `data: <json>\n\n` SSE message.
///
/// Lagged events (i.e. when a client falls behind by > `BROADCAST_CAPACITY`
/// entries) are silently skipped — the stream continues without interruption.
async fn sse_handler(
    tx: broadcast::Sender<String>,
) -> Sse<impl futures_lite::Stream<Item = Result<Event, Infallible>>> {
    let rx = tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        match result {
            Ok(json_line) => Some(Ok(Event::default().data(json_line))),
            // BroadcastStream::Lagged — skip silently, do not close the stream.
            Err(_) => None,
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
