//! Drives the JS shim package via Node. Spawns the Rust HTTP shim, then
//! `node --test` against `packages/tauri-api-shim/tests/m5_listen.test.mjs`.
//!
//! Skipped automatically if `node` isn't on PATH.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::Emitter;
use tokio::sync::oneshot;

#[tauri::command]
async fn trigger_greeting(app: tauri::AppHandle, msg: String) -> Result<(), String> {
    app.emit(
        "greeting",
        json!({ "from": "rust", "echoed": msg }),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn trigger_one_shot(app: tauri::AppHandle) -> Result<(), String> {
    app.emit("one-shot", Value::Null).map_err(|e| e.to_string())
}

#[tauri::command]
async fn emit_to_window(
    app: tauri::AppHandle,
    label: String,
    name: String,
) -> Result<(), String> {
    app.emit_to(label.as_str(), &name, Value::Null)
        .map_err(|e| e.to_string())
}

struct Shim {
    base: String,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<Result<(), tauri::Error>>>,
}

impl Shim {
    async fn spawn() -> Self {
        let server = tauri::Builder::default()
            .invoke_handler(tauri::generate_handler![
                trigger_greeting,
                trigger_one_shot,
                emit_to_window
            ])
            .bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = server.local_addr();
        let (tx, rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            server
                .serve_with_shutdown(async move {
                    let _ = rx.await;
                })
                .await
        });
        Self {
            base: format!("http://{addr}"),
            shutdown: Some(tx),
            join: Some(join),
        }
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.join.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), j).await;
        }
    }
}

fn workspace_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn js_shim_listen_round_trip() {
    if !node_available() {
        eprintln!("skipping: `node` not on PATH");
        return;
    }
    let shim = Shim::spawn().await;
    let workspace = workspace_dir();
    let test_path = workspace.join("packages/tauri-api-shim/tests/m5_listen.test.mjs");

    // EventSource is gated behind a flag in some Node versions; pass it.
    let output = tokio::process::Command::new("node")
        .current_dir(&workspace)
        .arg("--experimental-eventsource")
        .arg("--test")
        .arg(&test_path)
        .env("TAURI_DEBUG_BASE", &shim.base)
        .env("TAURI_DEBUG_WINDOW", "main")
        .output()
        .await
        .expect("spawn node");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("node test failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    }

    shim.shutdown().await;
}
