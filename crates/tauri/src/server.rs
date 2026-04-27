//! Axum HTTP server.

use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use futures_util::stream::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::{Any, CorsLayer};

use crate::InvokeHandlerFn;
use crate::event::{EventEnvelope, EventTarget, event_matches_listener, next_event_id};
use crate::ipc::{CommandRequest, InvokeError};
use crate::manager::{AppHandle, Wry};

const DEFAULT_WINDOW: &str = "main";

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) handler: InvokeHandlerFn,
    pub(crate) app_handle: AppHandle<Wry>,
}

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/__tauri/invoke/{cmd}", post(invoke))
        .route("/__tauri/events", get(events))
        .route("/__tauri/emit", post(emit_endpoint))
        .route("/__tauri/listen", post(listen_noop))
        .route("/__tauri/unlisten", post(listen_noop))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state)
}

async fn invoke(
    State(state): State<AppState>,
    Path(cmd): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body_value: Value = if body.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(v) => v,
            Err(e) => {
                let err = InvokeError::from_message(format!("invalid JSON body: {e}"));
                return error_response(StatusCode::BAD_REQUEST, err);
            }
        }
    };

    let window_label = window_from_headers(&headers);
    register_window(&state.app_handle, &window_label);

    let req = CommandRequest::new(cmd, body_value, state.app_handle.clone(), window_label);

    match (state.handler)(req).await {
        Ok(value) => (StatusCode::OK, axum::Json(value)).into_response(),
        Err(err) => error_response(StatusCode::UNPROCESSABLE_ENTITY, err),
    }
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    window: Option<String>,
    #[serde(rename = "clientId")]
    _client_id: Option<String>,
}

async fn events(
    State(state): State<AppState>,
    Query(params): Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send> {
    let window_label = params.window.unwrap_or_else(|| DEFAULT_WINDOW.to_string());
    register_window(&state.app_handle, &window_label);

    // Build a per-client EventTarget representing the listener identity.
    // SSE clients identify as `WebviewWindow` since user code most often
    // emits via `window.emit()` / `app.emit_to(label, ...)`.
    let listener = EventTarget::WebviewWindow {
        label: window_label.clone(),
    };
    let rx = state.app_handle.inner.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let listener = listener.clone();
        async move {
            let envelope = match item {
                Ok(env) => env,
                Err(_) => return None,
            };
            if !event_matches_listener(&envelope.target, &listener) {
                return None;
            }
            let event = build_sse_event(&envelope);
            Some(Ok(event))
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}

fn build_sse_event(envelope: &EventEnvelope) -> Event {
    // The data is a JSON object whose shape the JS shim parses. We send the
    // whole envelope so the JS side can dispatch by name and expose source.
    let data = serde_json::to_string(envelope).unwrap_or_else(|_| "{}".into());
    Event::default()
        .event(envelope.event.clone())
        .id(envelope.id.to_string())
        .data(data)
}

fn window_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-tauri-window")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_WINDOW)
        .to_string()
}

fn register_window(handle: &AppHandle<Wry>, label: &str) {
    let mut windows = handle
        .inner
        .windows
        .write()
        .expect("windows map poisoned");
    windows.entry(label.to_string()).or_insert(());
}

/// `POST /__tauri/emit` — frontend-originated emit.
///
/// Body: `{ event: string, payload?: any, target?: EventTarget }`. The
/// envelope's `source` is set to `WebviewWindow{label}` based on the
/// `X-Tauri-Window` header so backend listeners can tell where the event
/// came from.
async fn emit_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    #[derive(Deserialize)]
    struct EmitBody {
        event: String,
        #[serde(default)]
        payload: Option<Value>,
        #[serde(default)]
        target: Option<EventTarget>,
    }

    let parsed: EmitBody = if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty emit body").into_response();
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("invalid emit body: {e}"),
                )
                    .into_response();
            }
        }
    };

    let window_label = window_from_headers(&headers);
    register_window(&state.app_handle, &window_label);

    let envelope = EventEnvelope {
        id: next_event_id(),
        event: parsed.event,
        payload: parsed.payload.unwrap_or(Value::Null),
        target: parsed.target.unwrap_or(EventTarget::Any),
        source: EventTarget::WebviewWindow {
            label: window_label,
        },
    };
    let _ = state.app_handle.inner.events.send(envelope);

    StatusCode::OK.into_response()
}

/// No-op for parity with upstream's `__TAURI_EVENT_PLUGIN_INTERNALS__`
/// listen/unlisten accounting. The shim's SSE stream already carries every
/// event; the JS shim doesn't need to register listeners with the server.
async fn listen_noop() -> Response {
    StatusCode::OK.into_response()
}

fn error_response(status: StatusCode, err: InvokeError) -> Response {
    (status, axum::Json(err.into_value())).into_response()
}
