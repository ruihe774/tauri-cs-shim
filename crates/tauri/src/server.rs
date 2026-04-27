//! Axum HTTP server.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use bytes::Bytes;
use serde_json::Value;

use crate::InvokeHandlerFn;
use crate::ipc::{CommandRequest, InvokeError};
use crate::manager::{AppHandle, Wry};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) handler: InvokeHandlerFn,
    pub(crate) app_handle: AppHandle<Wry>,
}

pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/__tauri/invoke/{cmd}", post(invoke))
        .with_state(state)
}

async fn invoke(
    State(state): State<AppState>,
    Path(cmd): Path<String>,
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

    let req = CommandRequest::new(cmd, body_value, state.app_handle.clone());

    match (state.handler)(req).await {
        Ok(value) => (StatusCode::OK, axum::Json(value)).into_response(),
        Err(err) => error_response(StatusCode::UNPROCESSABLE_ENTITY, err),
    }
}

fn error_response(status: StatusCode, err: InvokeError) -> Response {
    (status, axum::Json(err.into_value())).into_response()
}
