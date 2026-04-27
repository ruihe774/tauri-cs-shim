//! Re-exports of tokio primitives under the names upstream Tauri uses.
//!
//! User code that imports `tauri::async_runtime::{spawn, Mutex, ...}` keeps
//! working. The shim's runtime is the standard tokio multi-thread runtime
//! that `Builder::run` constructs.

pub use tokio::sync::{Mutex, RwLock};
pub use tokio::task::{JoinHandle, spawn, spawn_blocking};

use std::future::Future;

/// Run `f` to completion. Uses the current runtime when one is active,
/// otherwise spins up a one-off `current_thread` runtime.
pub fn block_on<F: Future>(f: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(f),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create one-off tokio runtime")
            .block_on(f),
    }
}
