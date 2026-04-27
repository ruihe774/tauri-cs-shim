//! Debug shim for the `tauri` crate.
//!
//! M1 surface:
//! - `Builder::default().invoke_handler(...).run(generate_context!())`
//! - `#[tauri::command]` for sync/async functions with JSON-deserializable params
//! - `POST /__tauri/invoke/{cmd}` HTTP endpoint backed by Axum

pub mod ipc;
mod server;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use tauri_macros::{command, generate_context, generate_handler};

pub use ipc::{CommandRequest, InvokeError};

/// Stable type alias for the dispatch future returned by command wrappers.
pub type InvokeFuture =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, InvokeError>> + Send>>;

pub(crate) type InvokeHandlerFn =
    Arc<dyn Fn(CommandRequest) -> InvokeFuture + Send + Sync + 'static>;

/// The configuration produced by `tauri::generate_context!()`.
///
/// In real Tauri this carries the app config, asset bundle, and so on. The
/// shim replaces it with a stub: the user's frontend is served by their dev
/// server (or the static-file fallback), and we have no need for compiled-in
/// config for M1.
#[derive(Debug, Default, Clone)]
pub struct Context {
    _private: (),
}

impl Context {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    NoInvokeHandler,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::NoInvokeHandler => write!(f, "Builder::invoke_handler was never called"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::NoInvokeHandler => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::Io(value)
    }
}

#[derive(Default)]
pub struct Builder {
    invoke_handler: Option<InvokeHandlerFn>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the handler produced by `tauri::generate_handler!`.
    pub fn invoke_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(CommandRequest) -> InvokeFuture + Send + Sync + 'static,
    {
        self.invoke_handler = Some(Arc::new(handler));
        self
    }

    /// Bind the HTTP server to `addr` without serving it. Returns a [`Server`]
    /// the caller can drive on a tokio runtime — useful for tests that need
    /// the bound port (pass `"127.0.0.1:0"`).
    pub async fn bind(self, addr: impl tokio::net::ToSocketAddrs) -> Result<Server, Error> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok(Server {
            local_addr,
            listener,
            handler: self.invoke_handler,
        })
    }

    /// Start the HTTP server and block the current thread until shutdown.
    ///
    /// `_context` is accepted for API parity with upstream and is otherwise
    /// unused.
    pub fn run(self, _context: Context) -> Result<(), Error> {
        init_tracing();
        let host =
            std::env::var("TAURI_DEBUG_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = std::env::var("TAURI_DEBUG_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(1421);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        runtime.block_on(async move {
            let server = self.bind((host.as_str(), port)).await?;
            tracing::info!(addr = %server.local_addr, "tauri-cs-shim listening");
            server.serve().await
        })
    }
}

/// A bound but not-yet-serving HTTP server.
pub struct Server {
    pub local_addr: std::net::SocketAddr,
    listener: tokio::net::TcpListener,
    handler: Option<InvokeHandlerFn>,
}

impl Server {
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    /// Serve until the listener errors or the process exits.
    pub async fn serve(self) -> Result<(), Error> {
        let handler = self.handler.ok_or(Error::NoInvokeHandler)?;
        let router = server::router(server::AppState { handler });
        axum::serve(self.listener, router).await?;
        Ok(())
    }

    /// Serve with a graceful-shutdown signal — useful for tests.
    pub async fn serve_with_shutdown<F>(self, shutdown: F) -> Result<(), Error>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handler = self.handler.ok_or(Error::NoInvokeHandler)?;
        let router = server::router(server::AppState { handler });
        axum::serve(self.listener, router)
            .with_graceful_shutdown(shutdown)
            .await?;
        Ok(())
    }
}

fn init_tracing() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    });
}

#[doc(hidden)]
pub mod __private {
    //! Re-exports used by macro-generated code. Not a stable public API.
    pub use serde_json;
}
