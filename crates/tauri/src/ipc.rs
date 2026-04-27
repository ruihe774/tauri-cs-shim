//! IPC types shared between the HTTP server and macro-generated wrappers.

use serde_json::Value;

use crate::manager::{AppHandle, Manager, State, Window, Wry};

/// A single decoded `POST /__tauri/invoke/{cmd}` request, as seen by the
/// dispatch closure produced by `generate_handler!`.
#[derive(Debug, Clone)]
pub struct CommandRequest {
    name: String,
    body: Value,
    app_handle: AppHandle<Wry>,
    window_label: String,
}

impl CommandRequest {
    pub fn new(
        name: impl Into<String>,
        body: Value,
        app_handle: AppHandle<Wry>,
        window_label: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            body,
            app_handle,
            window_label: window_label.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn body(&self) -> &Value {
        &self.body
    }

    pub fn app_handle(&self) -> &AppHandle<Wry> {
        &self.app_handle
    }

    pub fn window_label(&self) -> &str {
        &self.window_label
    }

    /// Owned [`Window`] for the calling client.
    pub fn window(&self) -> Window<Wry> {
        Window::new(self.app_handle.clone(), self.window_label.clone())
    }
}

/// An error returned from a command handler. Serialized as the JSON body of a
/// `422 Unprocessable Entity` response.
#[derive(Debug, Clone)]
pub struct InvokeError(Value);

impl InvokeError {
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    pub fn from_message(msg: impl Into<String>) -> Self {
        Self(Value::String(msg.into()))
    }

    pub fn not_found(name: impl Into<String>) -> Self {
        let name = name.into();
        Self(serde_json::json!({
            "kind": "not_found",
            "command": name,
        }))
    }

    pub fn into_value(self) -> Value {
        self.0
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

impl std::fmt::Display for InvokeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for InvokeError {}

/// Trait used by the `#[command]` macro to extract each argument from a
/// [`CommandRequest`].
///
/// Mirrors upstream's `tauri::ipc::CommandArg`. The macro emits
/// `<T as CommandArg>::from_command(&req, key)` for every parameter and
/// trait dispatch picks the right extractor — JSON-deserialize for plain
/// serde types, type-map lookup for `State<'_, T>`, clone-from-request for
/// `AppHandle` / `Window` / `WebviewWindow`. Crucially this works through
/// type aliases (`type ManagedState<'a> = State<'a, T>`), which a
/// syntactic special-case in the macro could not handle.
pub trait CommandArg<'r>: Sized {
    fn from_command(req: &'r CommandRequest, key: &str) -> Result<Self, InvokeError>;
}

/// Blanket impl: any `DeserializeOwned` type — i.e. plain user payloads —
/// is fetched by name from the JSON body. This intentionally does NOT
/// cover the special types below (none of `State` / `AppHandle` / `Window`
/// implement `Deserialize`), so coherence treats the impls as disjoint.
impl<'r, T> CommandArg<'r> for T
where
    T: serde::de::DeserializeOwned,
{
    fn from_command(req: &'r CommandRequest, key: &str) -> Result<Self, InvokeError> {
        match req.body().get(key) {
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
                InvokeError::from_message(format!("invalid argument `{key}`: {e}"))
            }),
            None => Err(InvokeError::from_message(format!(
                "missing argument `{key}`"
            ))),
        }
    }
}

impl<'r, T: Send + Sync + 'static> CommandArg<'r> for State<'r, T> {
    fn from_command(req: &'r CommandRequest, _key: &str) -> Result<Self, InvokeError> {
        req.app_handle().try_state::<T>().ok_or_else(|| {
            InvokeError::from_message(format!(
                "state of type `{}` is not managed",
                std::any::type_name::<T>()
            ))
        })
    }
}

impl<'r> CommandArg<'r> for AppHandle<Wry> {
    fn from_command(req: &'r CommandRequest, _key: &str) -> Result<Self, InvokeError> {
        Ok(req.app_handle().clone())
    }
}

impl<'r> CommandArg<'r> for Window<Wry> {
    fn from_command(req: &'r CommandRequest, _key: &str) -> Result<Self, InvokeError> {
        Ok(req.window())
    }
}
