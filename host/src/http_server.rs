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
//! | Path             | Description                                           |
//! |------------------|-------------------------------------------------------|
//! | `GET /`          | Serves the embedded dashboard HTML page               |
//! | `GET /logo.png`  | Serves the embedded app icon (256 × 256 PNG)          |
//! | `GET /events`    | SSE stream — one JSONL event per `data:` message      |
//! | `GET /info`      | Current session state as JSON (planned, Issue D)      |
//! | `GET /shutdown`  | Graceful process exit (requires `X-RustyCAN-Shutdown`) |
//!
//! The server binds exclusively to `127.0.0.1` so it is never reachable
//! from outside the local machine.  HTTPS is unnecessary on loopback.
//!
//! # Multi-instance deployments
//!
//! Each simultaneously-running RustyCAN instance **must use a distinct port**.
//! Set `http_port` in each instance's config file, or pass `--http-port` on the
//! CLI.  The `/shutdown` takeover mechanism (see [`SseServer::start`]) sends a
//! graceful exit to any existing instance on the same port before binding.
//! When two instances accidentally share a port, the second kills the first.
//!
//! The planned `GET /info` endpoint will let the takeover logic check whether
//! an active session is running before sending `/shutdown`, preventing silent
//! session termination.
//!
//! # Session identity in the dashboard
//!
//! The SSE stream carries all JSONL log events.  The existing `session_start`
//! event (emitted by `EventLogger::log_session_start` at session open) already
//! carries the adapter name and baud rate.  Expanding it to include serial,
//! firmware version, and a dedicated sticky-header update in the dashboard JS
//! is tracked as Issue C.  Browser tabs that connect after session open will
//! receive the identity via `GET /info` once Issue D lands.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::header;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Extension, Json, Router};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

// ─── Embedded assets ─────────────────────────────────────────────────────────

/// The dashboard HTML page, compiled into the binary at build time.
const INDEX_HTML: &str = include_str!("../assets/index.html");

/// The app icon PNG, served at `/logo.png` for the browser dashboard.
const LOGO_PNG: &[u8] = include_bytes!("../assets/RustyCAN.iconset/icon_256x256.png");

/// MesloLGS NF font — same typeface used by the egui GUI title.
const MESLO_FONT: &[u8] = include_bytes!("../assets/MesloLGSNF-Regular.ttf");

// ─── Broadcast channel capacity ───────────────────────────────────────────────

/// Number of events buffered per subscriber before oldest are dropped.
///
/// At 500 events/sec a lagged client has ~200 ms to catch up before events
/// are silently dropped (lagged subscribers do not block the sender).
const BROADCAST_CAPACITY: usize = 128;

/// Session identity served at `GET /info` for late-joining dashboard tabs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub adapter_name: String,
    pub baud: u32,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub started_at_utc: String,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Handle to the running SSE HTTP server.
///
/// Clone `tx` and pass it to [`EventLogger::attach_sse`] to wire up live
/// streaming.  Call [`SseServer::set_session_info`] after a session opens so
/// late-joining browser tabs can get current session state from `GET /info`.
/// The server runs for the lifetime of this struct (dropping it does not stop
/// the background thread — it runs until the process exits).
pub struct SseServer {
    /// Broadcast sender — clone this to publish events from the logger.
    pub tx: broadcast::Sender<String>,
    session_info: Arc<std::sync::Mutex<Option<SessionInfo>>>,
}

impl SseServer {
    /// Populate the `/info` endpoint once a session has started.
    pub fn set_session_info(&self, info: SessionInfo) {
        *self.session_info.lock().unwrap() = Some(info);
    }

    /// Clear session info when the session ends (optional — `/info` returns 204).
    pub fn clear_session_info(&self) {
        *self.session_info.lock().unwrap() = None;
    }

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
        let session_info: Arc<std::sync::Mutex<Option<SessionInfo>>> =
            Arc::new(std::sync::Mutex::new(None));
        let info = session_info.clone();

        // ── Graceful takeover: shut down any existing instance on this port ──
        // If another RustyCAN is already running, send it /shutdown so it exits
        // and frees the port before this instance tries to bind.
        // The X-RustyCAN-Shutdown header is required by the handler so that a
        // browser page cannot trigger a cross-origin shutdown via a simple GET
        // (non-simple headers force a CORS pre-flight, which the server does
        // not allow, so the browser blocks such requests).
        let old_instance_found = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_millis(300)))
                .build(),
        )
        .get(&format!("http://127.0.0.1:{port}/shutdown"))
        .header("X-RustyCAN-Shutdown", "1")
        .call()
        .is_ok();
        if old_instance_found {
            // Give the old process time to call ExitProcess and release the port.
            std::thread::sleep(std::time::Duration::from_millis(600));
        }

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
                        .route("/font/meslo.ttf", get(serve_font))
                        .route("/events", get(move || sse_handler(tx_clone.clone())))
                        .route("/info", get(serve_info))
                        .route("/shutdown", get(handle_shutdown))
                        .layer(Extension(info));

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

        SseServer { tx, session_info }
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

/// Serve the MesloLGS NF font — used by the dashboard title to match the GUI.
async fn serve_font() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "font/ttf"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        MESLO_FONT,
    )
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

/// Serve session identity for late-joining dashboard browser tabs.
/// Returns 204 No Content when no session is currently active.
async fn serve_info(
    Extension(info): Extension<Arc<std::sync::Mutex<Option<SessionInfo>>>>,
) -> impl IntoResponse {
    match info.lock().unwrap().clone() {
        Some(si) => Json(si).into_response(),
        None => axum::http::StatusCode::NO_CONTENT.into_response(),
    }
}

/// Shut down this process after sending the HTTP response.
///
/// Called by a newly-started RustyCAN instance to take over the port.
/// A background thread exits the process ~100 ms after this handler returns,
/// giving axum enough time to flush the response to the caller.
///
/// # Security
///
/// The `X-RustyCAN-Shutdown: 1` header is **required**.  Without it the
/// handler returns 403.  Custom headers are not "simple" under the CORS
/// specification, so any cross-origin browser request must first send a
/// pre-flight OPTIONS — which this server does not permit — ensuring that
/// a malicious webpage cannot trigger a shutdown via a bare fetch/XHR.
async fn handle_shutdown(headers: axum::http::HeaderMap) -> impl IntoResponse {
    if headers
        .get("x-rustycan-shutdown")
        .and_then(|v| v.to_str().ok())
        != Some("1")
    {
        return (axum::http::StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        eprintln!("[rustycan] Shutting down: new instance is taking over.");
        std::process::exit(0);
    });
    (axum::http::StatusCode::OK, "shutting down").into_response()
}
