//! Runtime, App, AppHandle, StateManager, State, and the Manager trait.
//!
//! This module exists in M3 in skeleton form. M4 grows window/runtime methods
//! on top; M5/M6 extend `AppInner` with the event bus and listener registry.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::{Arc, RwLock};

/// Marker trait used as a type parameter on [`AppHandle`] etc., for parity
/// with upstream Tauri. The shim never dispatches on it.
pub trait Runtime: Send + Sync + 'static {}

/// Default [`Runtime`]. Upstream's Wry-backed runtime; here a no-op marker.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wry;
impl Runtime for Wry {}

/// Type-keyed bag of `Send + Sync + 'static` values, owned by `AppInner`.
#[derive(Default)]
pub struct StateManager {
    inner: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl StateManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a state value. Returns `false` if a value of the same type was
    /// already managed (parity with upstream behaviour).
    pub fn manage<T: Send + Sync + 'static>(&self, state: T) -> bool {
        let mut map = self.inner.write().expect("state map poisoned");
        let id = TypeId::of::<T>();
        if map.contains_key(&id) {
            return false;
        }
        map.insert(id, Arc::new(state));
        true
    }

    pub fn try_get<T: Send + Sync + 'static>(&self) -> Option<State<'static, T>> {
        let map = self.inner.read().expect("state map poisoned");
        let any = map.get(&TypeId::of::<T>())?.clone();
        let arc = any.downcast::<T>().ok()?;
        Some(State {
            inner: arc,
            _lt: PhantomData,
        })
    }

    pub fn unmanage<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let mut map = self.inner.write().expect("state map poisoned");
        map.remove(&TypeId::of::<T>())
            .and_then(|arc| arc.downcast::<T>().ok())
    }
}

/// Borrowed reference to a managed state value.
///
/// Internally holds an `Arc<T>`, so the lifetime parameter is API-shape-only;
/// `State<'r, T>` is observationally `'static` as long as the value remains
/// managed. This deviates from upstream's exact representation but is
/// indistinguishable to the user.
pub struct State<'r, T: ?Sized + 'static> {
    inner: Arc<T>,
    _lt: PhantomData<&'r T>,
}

impl<T: ?Sized + 'static> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _lt: PhantomData,
        }
    }
}

impl<T: ?Sized + 'static> Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: ?Sized + std::fmt::Debug + 'static> std::fmt::Debug for State<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("State").field(&&*self.inner).finish()
    }
}

impl<T: 'static> State<'_, T> {
    pub fn inner(&self) -> &T {
        &self.inner
    }
}

/// Internal app state shared by every clone of [`AppHandle`].
///
/// M5/M6 extend this with an event broadcaster and listener registry.
pub struct AppInner {
    pub(crate) state: StateManager,
    pub(crate) windows: RwLock<HashMap<String, ()>>,
}

impl AppInner {
    pub(crate) fn new() -> Self {
        let mut windows = HashMap::new();
        windows.insert("main".to_string(), ());
        Self {
            state: StateManager::new(),
            windows: RwLock::new(windows),
        }
    }
}

/// Cheaply-clonable handle to the running app.
pub struct AppHandle<R: Runtime = Wry> {
    pub(crate) inner: Arc<AppInner>,
    pub(crate) _r: PhantomData<R>,
}

impl<R: Runtime> Clone for AppHandle<R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _r: PhantomData,
        }
    }
}

impl<R: Runtime> std::fmt::Debug for AppHandle<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppHandle").finish_non_exhaustive()
    }
}

impl AppHandle<Wry> {
    /// Best-effort process exit. Mirrors `tauri::AppHandle::exit`.
    pub fn exit(&self, code: i32) -> ! {
        std::process::exit(code)
    }
}

/// Owned handle held by `Builder::setup`. In upstream Tauri this is distinct
/// from `AppHandle` and carries the run-loop control flow; here it simply
/// wraps an `AppHandle`.
pub struct App<R: Runtime = Wry> {
    pub(crate) handle: AppHandle<R>,
}

impl<R: Runtime> App<R> {
    pub fn handle(&self) -> &AppHandle<R> {
        &self.handle
    }
}

impl<R: Runtime> std::fmt::Debug for App<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App").finish_non_exhaustive()
    }
}

/// Logical window in the shim. Each connected client identifies its window
/// via the `X-Tauri-Window` header (default `"main"`). Window-side methods
/// like `set_title` are no-ops; we never owned a real window to begin with.
pub struct Window<R: Runtime = Wry> {
    handle: AppHandle<R>,
    label: String,
}

/// Type alias per the design decision — `WebviewWindow` and `Window` are the
/// same type in the shim.
pub type WebviewWindow<R = Wry> = Window<R>;

impl<R: Runtime> Window<R> {
    pub(crate) fn new(handle: AppHandle<R>, label: String) -> Self {
        Self { handle, label }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// No-op stub. Logged at debug level so the user sees the call.
    pub fn set_title(&self, title: &str) -> Result<(), crate::Error> {
        tracing::debug!(window = %self.label, ?title, "Window::set_title (shim no-op)");
        Ok(())
    }

    pub fn show(&self) -> Result<(), crate::Error> {
        tracing::debug!(window = %self.label, "Window::show (shim no-op)");
        Ok(())
    }

    pub fn hide(&self) -> Result<(), crate::Error> {
        tracing::debug!(window = %self.label, "Window::hide (shim no-op)");
        Ok(())
    }

    pub fn close(&self) -> Result<(), crate::Error> {
        tracing::debug!(window = %self.label, "Window::close (shim no-op)");
        Ok(())
    }

    pub fn is_focused(&self) -> Result<bool, crate::Error> {
        Ok(true)
    }

    pub fn is_visible(&self) -> Result<bool, crate::Error> {
        Ok(true)
    }

    pub fn is_minimized(&self) -> Result<bool, crate::Error> {
        Ok(false)
    }

    pub fn is_maximized(&self) -> Result<bool, crate::Error> {
        Ok(false)
    }
}

impl<R: Runtime> Clone for Window<R> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            label: self.label.clone(),
        }
    }
}

impl<R: Runtime> std::fmt::Debug for Window<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Window")
            .field("label", &self.label)
            .finish()
    }
}

impl<R: Runtime> Manager<R> for Window<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        &self.handle
    }
}

/// Stub `Config` returned by `Manager::config()`. Real Tauri returns the
/// parsed `tauri.conf.json`; the shim has no such file.
#[derive(Debug, Default, Clone)]
pub struct Config {
    _private: (),
}

impl Config {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

/// Trait giving access to managed state and (M4+) windows.
///
/// Real Tauri implements this on `App`, `AppHandle`, `Window`, and
/// `WebviewWindow`. The shim follows the same shape; methods that aren't
/// meaningful in a debug-shim context (window enumeration, config) are stubs.
pub trait Manager<R: Runtime>: Sized {
    fn app_handle(&self) -> &AppHandle<R>;

    fn manage<T: Send + Sync + 'static>(&self, state: T) -> bool {
        self.app_handle().inner.state.manage(state)
    }

    fn unmanage<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.app_handle().inner.state.unmanage::<T>()
    }

    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        self.try_state()
            .unwrap_or_else(|| panic!("state of type `{}` is not managed", std::any::type_name::<T>()))
    }

    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
        self.app_handle().inner.state.try_get::<T>()
    }

    /// Fetch a window by label. Returns `None` if no client has identified
    /// itself with that label.
    fn get_webview_window(&self, label: &str) -> Option<WebviewWindow<R>> {
        let windows = self
            .app_handle()
            .inner
            .windows
            .read()
            .expect("windows map poisoned");
        if windows.contains_key(label) {
            Some(Window::new(self.app_handle().clone(), label.to_string()))
        } else {
            None
        }
    }

    /// Snapshot of every window the shim currently knows about.
    fn webview_windows(&self) -> HashMap<String, WebviewWindow<R>> {
        let windows = self
            .app_handle()
            .inner
            .windows
            .read()
            .expect("windows map poisoned");
        windows
            .keys()
            .map(|label| {
                (
                    label.clone(),
                    Window::new(self.app_handle().clone(), label.clone()),
                )
            })
            .collect()
    }

    /// Stub configuration. Real Tauri returns the compiled-in
    /// `tauri.conf.json`; the shim has none.
    fn config(&self) -> Config {
        Config::new()
    }
}

impl<R: Runtime> Manager<R> for AppHandle<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        self
    }
}

impl<R: Runtime> Manager<R> for App<R> {
    fn app_handle(&self) -> &AppHandle<R> {
        &self.handle
    }
}
