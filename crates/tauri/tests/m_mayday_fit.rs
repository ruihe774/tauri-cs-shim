//! Coverage gaps surfaced by surveying the mayday sample project.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Manager;
use tokio::sync::oneshot;

#[derive(Default)]
struct Counter(AtomicI64);

#[tauri::command]
fn add_units(state: tauri::State<'_, Counter>, by_count: i64, idempotency_key: String) -> i64 {
    if idempotency_key.is_empty() {
        return state.0.load(Ordering::SeqCst);
    }
    state.0.fetch_add(by_count, Ordering::SeqCst);
    state.0.load(Ordering::SeqCst)
}

#[derive(Deserialize, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct AcquireAircraftPayload {
    aircraft_type_icao: String,
    acquisition_mode: String,
    nickname_override: Option<String>,
}

#[tauri::command]
fn echo_acquire(payload: AcquireAircraftPayload) -> AcquireAircraftPayload {
    payload
}

#[tauri::command]
async fn ping_via_main_thread(app: tauri::AppHandle) -> Result<String, String> {
    // Bounce a closure through run_on_main_thread to confirm it executes.
    let h = app.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = h; // keep the AppHandle alive inside the closure
        let _ = tx.send("pong");
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(1))
        .map(|s| s.to_string())
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
            .manage(Counter::default())
            .invoke_handler(tauri::generate_handler![
                add_units,
                echo_acquire,
                ping_via_main_thread
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

#[tokio::test]
async fn multiword_params_use_camelcase_keys() {
    let shim = Shim::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/add_units", shim.base))
        .json(&json!({"byCount": 5, "idempotencyKey": "k1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<serde_json::Value>().await.unwrap(), json!(5));
    shim.shutdown().await;
}

#[tokio::test]
async fn payload_struct_uses_struct_serde_attrs() {
    let shim = Shim::spawn().await;
    // The macro routes the parameter named `payload` to the JSON key
    // "payload" and lets serde decode the inner struct using its own
    // rename_all = "camelCase". Confirm a typical mayday-shaped payload
    // round-trips.
    let body = json!({
        "payload": {
            "aircraftTypeIcao": "A320",
            "acquisitionMode": "purchase",
            "nicknameOverride": "Kestrel"
        }
    });
    let got: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/echo_acquire", shim.base))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got, body["payload"]);
    shim.shutdown().await;
}

#[tokio::test]
async fn run_on_main_thread_executes_closure() {
    let shim = Shim::spawn().await;
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/ping_via_main_thread", shim.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, json!("pong"));
    shim.shutdown().await;
}

#[tokio::test]
async fn run_on_main_thread_direct_handle() {
    // Exercise the API without HTTP.
    let server = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![add_units])
        .bind("127.0.0.1:0")
        .await
        .unwrap();
    let h = server.app_handle().clone();
    let counter = Arc::new(AtomicI64::new(0));
    let c = counter.clone();
    h.run_on_main_thread(move || {
        c.fetch_add(7, Ordering::SeqCst);
    })
    .unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 7);
}
