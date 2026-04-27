//! Drives the JS shim's `event.emit` / `emitTo` against backend listeners.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tauri::Listener;
use tokio::sync::oneshot;

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<(String, Value)>>>);

#[tauri::command]
fn show_recorded(state: tauri::State<'_, Recorded>) -> Vec<(String, Value)> {
    state.0.lock().unwrap().clone()
}

struct Shim {
    base: String,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<Result<(), tauri::Error>>>,
}

impl Shim {
    async fn spawn() -> Self {
        let recorded = Recorded::default();
        let server = tauri::Builder::default()
            .manage(recorded.clone())
            .setup({
                let recorded = recorded.clone();
                move |app| {
                    // App-scoped listeners. Per routing rules they match
                    // `Any`- and `App`-target events but NOT `AnyLabel`.
                    let r = recorded.clone();
                    app.listen("greeting", move |ev| {
                        r.0.lock().unwrap().push(("greeting".into(), ev.payload));
                    });
                    let r = recorded.clone();
                    app.listen("targeted", move |ev| {
                        r.0.lock().unwrap().push(("targeted".into(), ev.payload));
                    });
                    Ok(())
                }
            })
            .invoke_handler(tauri::generate_handler![show_recorded])
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
async fn js_shim_emit_round_trip() {
    if !node_available() {
        eprintln!("skipping: `node` not on PATH");
        return;
    }
    let shim = Shim::spawn().await;
    let workspace = workspace_dir();
    let test_path = workspace.join("packages/tauri-api-shim/tests/m6_emit.test.mjs");

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
