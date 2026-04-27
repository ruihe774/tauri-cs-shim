//! IPC types shared between the HTTP server and macro-generated wrappers.

use serde_json::Value;

use crate::manager::{AppHandle, Window, Wry};

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
