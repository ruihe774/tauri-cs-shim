//! Debug shim for the `tauri` crate.
//!
//! See `docs/tauri-debug-shim-plan.md` for the full surface roadmap.

pub mod async_runtime;
pub mod ipc;
mod manager;
mod server;

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

pub use tauri_macros::{command, generate_context, generate_handler};

pub use ipc::{CommandRequest, InvokeError};
pub use manager::{
    App, AppHandle, Config, Manager, Runtime, State, StateManager, WebviewWindow, Window, Wry,
};

use manager::AppInner;

/// Stable type alias for the dispatch future returned by command wrappers.
pub type InvokeFuture =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, InvokeError>> + Send>>;

pub(crate) type InvokeHandlerFn =
    Arc<dyn Fn(CommandRequest) -> InvokeFuture + Send + Sync + 'static>;

type SetupFn = Box<
    dyn FnOnce(&mut App<Wry>) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + Send
        + 'static,
>;

/// The configuration produced by `tauri::generate_context!()`.
///
/// In real Tauri this carries the app config, asset bundle, and so on. The
/// shim replaces it with a stub: the user's frontend is served by their dev
/// server (or the static-file fallback), and we have no need for compiled-in
/// config in the shim.
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
    Setup(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::NoInvokeHandler => write!(f, "Builder::invoke_handler was never called"),
            Error::Setup(e) => write!(f, "setup hook failed: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::NoInvokeHandler => None,
            Error::Setup(e) => Some(&**e),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::Io(value)
    }
}

pub struct Builder {
    inner: Arc<AppInner>,
    invoke_handler: Option<InvokeHandlerFn>,
    setup: Option<SetupFn>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            inner: Arc::new(AppInner::new()),
            invoke_handler: None,
            setup: None,
        }
    }
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stash a value of type `T` so that commands and `Manager::state::<T>()`
    /// can retrieve it. Subsequent attempts to manage the same type are
    /// ignored (parity with upstream).
    pub fn manage<T: Send + Sync + 'static>(self, state: T) -> Self {
        self.inner.state.manage(state);
        self
    }

    /// Register the handler produced by `tauri::generate_handler!`.
    pub fn invoke_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(CommandRequest) -> InvokeFuture + Send + Sync + 'static,
    {
        self.invoke_handler = Some(Arc::new(handler));
        self
    }

    /// One-shot setup hook executed after the app is built but before the
    /// HTTP server starts serving. Useful for late state registration.
    pub fn setup<F>(mut self, setup: F) -> Self
    where
        F: FnOnce(&mut App<Wry>) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
            + Send
            + 'static,
    {
        self.setup = Some(Box::new(setup));
        self
    }

    /// Bind the HTTP server to `addr` without serving it. Returns a [`Server`]
    /// the caller can drive on a tokio runtime — useful for tests that need
    /// the bound port (pass `"127.0.0.1:0"`).
    pub async fn bind(mut self, addr: impl tokio::net::ToSocketAddrs) -> Result<Server, Error> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;

        let handle = AppHandle {
            inner: self.inner.clone(),
            _r: PhantomData,
        };

        if let Some(setup) = self.setup.take() {
            let mut app = App {
                handle: handle.clone(),
            };
            setup(&mut app).map_err(Error::Setup)?;
        }

        Ok(Server {
            local_addr,
            listener,
            handle,
            invoke_handler: self.invoke_handler,
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
    handle: AppHandle<Wry>,
    invoke_handler: Option<InvokeHandlerFn>,
}

impl Server {
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    pub fn app_handle(&self) -> &AppHandle<Wry> {
        &self.handle
    }

    fn make_state(&self) -> Result<server::AppState, Error> {
        Ok(server::AppState {
            handler: self
                .invoke_handler
                .clone()
                .ok_or(Error::NoInvokeHandler)?,
            app_handle: self.handle.clone(),
        })
    }

    /// Serve until the listener errors or the process exits.
    pub async fn serve(self) -> Result<(), Error> {
        let state = self.make_state()?;
        let router = server::router(state);
        axum::serve(self.listener, router).await?;
        Ok(())
    }

    /// Serve with a graceful-shutdown signal — useful for tests.
    pub async fn serve_with_shutdown<F>(self, shutdown: F) -> Result<(), Error>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let state = self.make_state()?;
        let router = server::router(state);
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
