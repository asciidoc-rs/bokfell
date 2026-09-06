//! Development server for the Bokfell documentation site generator.
//!
//! `serve` watches the site's local inputs, rebuilds into memory on
//! change, and pushes a reload signal to connected browsers over a
//! WebSocket (PLAN.md §9.3). Design notes, learned from Zola's serve
//! implementation (PLAN.md §7):
//!
//! - **State is owned, not global**: the in-memory site map and the reload
//!   channel live in a [`ServeState`] the caller drops when done, so multiple
//!   servers can coexist (and tests can drive one directly).
//! - **Rebuilds are whole-site**: partial rebuilds without dependency tracking
//!   cause staleness bugs; a full rebuild of a documentation component is fast
//!   enough for a sub-second edit→reload loop. The dependency-graph
//!   incrementality planned for later milestones slots in behind
//!   [`SiteBuilder`] without changing this crate's surface.
//! - **A failed rebuild keeps the last good site** and reports the error; the
//!   browser keeps working.
//!
//! The generator side stays out of this crate: the caller supplies a
//! [`SiteBuilder`] closure that produces the full site as `url → bytes`,
//! and the list of paths to watch.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as UrlPath, State,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tokio::sync::broadcast;

/// The in-memory site: site-root-relative URL → file bytes.
pub type SiteMap = HashMap<String, Vec<u8>>;

/// Builds the whole site into memory. Called for the initial build and
/// again after every debounced filesystem change.
pub type SiteBuilder = Box<dyn FnMut() -> Result<SiteMap, String> + Send>;

/// Errors from running the dev server.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The initial site build failed (later failures keep the server
    /// running on the last good site).
    #[error("initial build failed: {0}")]
    InitialBuild(String),

    /// The file watcher could not be started.
    #[error("cannot watch {path}: {source}")]
    Watch {
        /// The path being watched.
        path: String,
        /// The underlying watcher error.
        source: notify_debouncer_full::notify::Error,
    },

    /// The server socket could not be bound or served.
    #[error("server error: {0}")]
    Server(#[from] std::io::Error),
}

/// Options for [`serve`].
pub struct ServeOptions {
    /// The address to bind (e.g. `127.0.0.1:8000`).
    pub addr: SocketAddr,

    /// Paths (files or directories) whose changes trigger a rebuild.
    pub watch: Vec<PathBuf>,

    /// Debounce window for filesystem events.
    pub debounce: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        ServeOptions {
            addr: ([127, 0, 0, 1], 8000).into(),
            watch: Vec::new(),
            debounce: Duration::from_millis(300),
        }
    }
}

/// Shared server state: the current site and the reload broadcast.
pub struct ServeState {
    site: RwLock<SiteMap>,
    reload: broadcast::Sender<()>,
}

impl ServeState {
    /// Creates the state with an initial site.
    pub fn new(site: SiteMap) -> Arc<Self> {
        let (reload, _) = broadcast::channel(16);
        Arc::new(ServeState {
            site: RwLock::new(site),
            reload,
        })
    }

    /// Swaps in a newly built site and tells connected browsers to
    /// reload.
    pub fn publish(&self, site: SiteMap) {
        *self.site.write().expect("site lock") = site;

        // No receivers is fine — nobody is watching yet.
        let _ = self.reload.send(());
    }

    /// Serves one URL from the current site, with the reload client
    /// injected into HTML documents.
    fn lookup(&self, path: &str) -> Option<(Vec<u8>, &'static str)> {
        let mut key = path.trim_start_matches('/').to_string();
        if key.is_empty() || key.ends_with('/') {
            key.push_str("index.html");
        }

        let site = self.site.read().expect("site lock");
        let bytes = site.get(&key)?;

        if key.ends_with(".html") {
            Some((inject_reload_client(bytes), "text/html; charset=utf-8"))
        } else {
            Some((bytes.clone(), content_type_for(&key)))
        }
    }
}

/// Runs the dev server until the process is terminated.
///
/// Performs the initial build with `builder`, then rebuilds on every
/// debounced change under `options.watch`.
pub fn serve(mut builder: SiteBuilder, options: ServeOptions) -> Result<(), ServeError> {
    let initial = builder().map_err(ServeError::InitialBuild)?;
    let state = ServeState::new(initial);

    // The watcher thread debounces filesystem events and rebuilds. The
    // debouncer must outlive the loop, so it moves into the thread.
    let rebuild_state = state.clone();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<()>();
    let mut debouncer = notify_debouncer_full::new_debouncer(options.debounce, None, {
        move |result: notify_debouncer_full::DebounceEventResult| {
            // Any effective event triggers a rebuild; classification and
            // dependency-tracked invalidation arrive in later milestones.
            if result.map(|events| !events.is_empty()).unwrap_or(false) {
                let _ = event_tx.send(());
            }
        }
    })
    .map_err(|source| ServeError::Watch {
        path: "<watcher>".to_string(),
        source,
    })?;

    for path in &options.watch {
        if !path.exists() {
            continue;
        }
        debouncer
            .watch(
                path,
                notify_debouncer_full::notify::RecursiveMode::Recursive,
            )
            .map_err(|source| ServeError::Watch {
                path: path.display().to_string(),
                source,
            })?;
    }

    std::thread::spawn(move || {
        // Keep the debouncer alive for the life of the loop.
        let _debouncer = debouncer;
        while event_rx.recv().is_ok() {
            // Coalesce events that arrived while a rebuild was running.
            while event_rx.try_recv().is_ok() {}

            let started = std::time::Instant::now();
            match builder() {
                Ok(site) => {
                    rebuild_state.publish(site);
                    eprintln!("rebuilt in {:.0?}", started.elapsed());
                }
                Err(message) => {
                    eprintln!("rebuild failed (serving last good site): {message}");
                }
            }
        }
    });

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let app = Router::new()
            .route("/", get(serve_root))
            .route("/__bokfell/reload", get(reload_socket))
            .route("/{*path}", get(serve_path))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(options.addr).await?;
        eprintln!("serving on http://{}/", options.addr);
        axum::serve(listener, app).await
    })?;

    Ok(())
}

async fn serve_root(State(state): State<Arc<ServeState>>) -> Response {
    respond(&state, "")
}

async fn serve_path(
    State(state): State<Arc<ServeState>>,
    UrlPath(path): UrlPath<String>,
) -> Response {
    respond(&state, &path)
}

fn respond(state: &ServeState, path: &str) -> Response {
    match state.lookup(path) {
        Some((bytes, content_type)) => {
            ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            format!("bokfell serve: no such page: /{path}\n"),
        )
            .into_response(),
    }
}

async fn reload_socket(
    State(state): State<Arc<ServeState>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let receiver = state.reload.subscribe();
    upgrade.on_upgrade(move |socket| reload_loop(socket, receiver))
}

async fn reload_loop(mut socket: WebSocket, mut receiver: broadcast::Receiver<()>) {
    while receiver.recv().await.is_ok() {
        if socket.send(Message::Text("reload".into())).await.is_err() {
            break;
        }
    }
}

/// The reload client injected into served HTML pages: reload on message,
/// and poll for the server after a disconnect (a restart or laptop
/// suspend), reloading once it is back.
const RELOAD_CLIENT: &str = "<script>(function(){\
var proto=location.protocol===\"https:\"?\"wss\":\"ws\";\
var sock=new WebSocket(proto+\"://\"+location.host+\"/__bokfell/reload\");\
sock.onmessage=function(){location.reload();};\
sock.onclose=function(){setTimeout(function retry(){\
fetch(location.href,{cache:\"no-store\"}).then(function(){location.reload();})\
.catch(function(){setTimeout(retry,1000);});},1000);};\
})();</script>";

fn inject_reload_client(html: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(html);
    match text.rfind("</body>") {
        Some(pos) => {
            let mut out = String::with_capacity(text.len() + RELOAD_CLIENT.len());
            out.push_str(&text[..pos]);
            out.push_str(RELOAD_CLIENT);
            out.push_str(&text[pos..]);
            out.into_bytes()
        }
        None => {
            let mut out = text.into_owned();
            out.push_str(RELOAD_CLIENT);
            out.into_bytes()
        }
    }
}

fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "pdf" => "application/pdf",
        "txt" | "adoc" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_serves_index_and_injects_reload_client() {
        let mut site = SiteMap::new();
        site.insert(
            "index.html".to_string(),
            b"<html><body>Hi</body></html>".to_vec(),
        );
        site.insert("_/style.css".to_string(), b"body{}".to_vec());
        let state = ServeState::new(site);

        let (bytes, ct) = state.lookup("").unwrap();
        assert_eq!(ct, "text/html; charset=utf-8");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("__bokfell/reload"));
        assert!(text.ends_with("</body></html>"));

        let (bytes, ct) = state.lookup("/_/style.css").unwrap();
        assert_eq!(ct, "text/css; charset=utf-8");
        assert_eq!(bytes, b"body{}");

        assert!(state.lookup("missing.html").is_none());
    }

    #[test]
    fn publish_swaps_the_site() {
        let state = ServeState::new(SiteMap::new());
        assert!(state.lookup("page.html").is_none());

        let mut site = SiteMap::new();
        site.insert("page.html".to_string(), b"<body>x</body>".to_vec());
        state.publish(site);
        assert!(state.lookup("page.html").is_some());
    }
}
