//! M5: backend → frontend events. Exercised here directly against the SSE
//! endpoint via reqwest. The JS shim has its own Node test.

use std::time::Duration;

use serde_json::{Value, json};
use tauri::Emitter;
use tokio::sync::oneshot;

#[tauri::command]
async fn fire_app_event(app: tauri::AppHandle, name: String, payload: Value) -> Result<(), String> {
    app.emit(&name, payload).map_err(|e| e.to_string())
}

#[tauri::command]
async fn fire_window_event(
    window: tauri::Window,
    name: String,
    payload: Value,
) -> Result<(), String> {
    window.emit(&name, payload).map_err(|e| e.to_string())
}

#[tauri::command]
async fn fire_to_label(
    app: tauri::AppHandle,
    label: String,
    name: String,
    payload: Value,
) -> Result<(), String> {
    app.emit_to(label.as_str(), &name, payload)
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
                fire_app_event,
                fire_window_event,
                fire_to_label
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

/// One parsed SSE message. Sufficient for the assertions we care about.
#[derive(Debug, Clone)]
struct SseRecord {
    event: String,
    data: String,
}

/// Open the SSE stream for `window` and read until `count` records or 2s.
/// Signals `ready` once the GET response headers are in (i.e. the server's
/// SSE handler has already called `subscribe`), so the caller can emit
/// without racing the subscription.
async fn drain_sse(
    base: &str,
    window: &str,
    count: usize,
    ready: tokio::sync::oneshot::Sender<()>,
) -> Vec<SseRecord> {
    let url = format!("{base}/__tauri/events?window={window}");
    let resp = reqwest::Client::new()
        .get(url)
        .header("accept", "text/event-stream")
        .send()
        .await
        .expect("sse get");
    assert_eq!(resp.status(), 200);
    // Headers received → handler has run → subscribe() already happened.
    let _ = ready.send(());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut buf = String::new();
    let mut records = Vec::new();
    let stream = resp.bytes_stream();
    use futures_util::StreamExt;
    tokio::pin!(stream);
    while records.len() < count && tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
        let Ok(Some(chunk)) = next else { continue };
        let Ok(chunk) = chunk else { break };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buf.find("\n\n") {
            let raw = buf[..end].to_string();
            buf.drain(..end + 2);
            if let Some(rec) = parse_sse_record(&raw) {
                records.push(rec);
            }
        }
    }
    records
}

fn parse_sse_record(raw: &str) -> Option<SseRecord> {
    let mut event = String::new();
    let mut data = String::new();
    let mut had_data = false;
    for line in raw.lines() {
        if line.starts_with(':') || line.is_empty() {
            continue; // comment or blank
        }
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if had_data {
                data.push('\n');
            }
            data.push_str(v.trim_start());
            had_data = true;
        }
    }
    if had_data {
        Some(SseRecord { event, data })
    } else {
        None
    }
}

async fn fire(client: &reqwest::Client, base: &str, cmd: &str, body: serde_json::Value) {
    let r = client
        .post(format!("{base}/__tauri/invoke/{cmd}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "command {cmd} failed: {}",
        r.status()
    );
}

#[tokio::test]
async fn app_emit_reaches_main_window_sse() {
    let shim = Shim::spawn().await;
    let (ready_tx, ready_rx) = oneshot::channel();
    let drain = tokio::spawn({
        let base = shim.base.clone();
        async move { drain_sse(&base, "main", 1, ready_tx).await }
    });
    ready_rx.await.unwrap();

    fire(
        &reqwest::Client::new(),
        &shim.base,
        "fire_app_event",
        json!({"name": "ping", "payload": {"v": 1}}),
    )
    .await;

    let records = drain.await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event, "ping");
    let parsed: serde_json::Value = serde_json::from_str(&records[0].data).unwrap();
    assert_eq!(parsed["event"], "ping");
    assert_eq!(parsed["payload"], json!({"v": 1}));
    assert_eq!(parsed["target"]["kind"], "Any");
    assert_eq!(parsed["source"]["kind"], "App");

    shim.shutdown().await;
}

#[tokio::test]
async fn emit_to_label_reaches_only_that_window() {
    let shim = Shim::spawn().await;
    let (rx_tx_main, rx_main) = oneshot::channel();
    let (rx_tx_settings, rx_settings) = oneshot::channel();
    let drain_main = tokio::spawn({
        let base = shim.base.clone();
        async move { drain_sse(&base, "main", 5, rx_tx_main).await }
    });
    let drain_settings = tokio::spawn({
        let base = shim.base.clone();
        async move { drain_sse(&base, "settings", 5, rx_tx_settings).await }
    });
    rx_main.await.unwrap();
    rx_settings.await.unwrap();

    let client = reqwest::Client::new();
    fire(
        &client,
        &shim.base,
        "fire_to_label",
        json!({"label": "settings", "name": "tick", "payload": "hi"}),
    )
    .await;
    fire(
        &client,
        &shim.base,
        "fire_to_label",
        json!({"label": "main", "name": "tock", "payload": "yo"}),
    )
    .await;

    // Each drain has a 2s deadline; both will return what they got by then.
    let main_records = tokio::time::timeout(Duration::from_secs(3), drain_main)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let settings_records = tokio::time::timeout(Duration::from_secs(3), drain_settings)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

    let main_events: Vec<_> = main_records.iter().map(|r| r.event.as_str()).collect();
    let settings_events: Vec<_> = settings_records
        .iter()
        .map(|r| r.event.as_str())
        .collect();
    assert!(
        main_events.contains(&"tock"),
        "main got: {main_events:?}"
    );
    assert!(
        !main_events.contains(&"tick"),
        "main got: {main_events:?}"
    );
    assert!(
        settings_events.contains(&"tick"),
        "settings got: {settings_events:?}"
    );
    assert!(
        !settings_events.contains(&"tock"),
        "settings got: {settings_events:?}"
    );

    shim.shutdown().await;
}

#[tokio::test]
async fn window_emit_reports_window_source() {
    let shim = Shim::spawn().await;
    let (ready_tx, ready_rx) = oneshot::channel();
    let drain = tokio::spawn({
        let base = shim.base.clone();
        async move { drain_sse(&base, "main", 1, ready_tx).await }
    });
    ready_rx.await.unwrap();

    let client = reqwest::Client::new();
    let r = client
        .post(format!("{}/__tauri/invoke/fire_window_event", shim.base))
        .header("X-Tauri-Window", "main")
        .json(&json!({"name": "wave", "payload": null}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    let records = drain.await.unwrap();
    assert_eq!(records.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&records[0].data).unwrap();
    assert_eq!(parsed["source"]["kind"], "Window");
    assert_eq!(parsed["source"]["label"], "main");

    shim.shutdown().await;
}

#[tokio::test]
async fn lagged_subscribers_are_skipped_not_fatal() {
    // A slow SSE consumer must not stall fast emitters or break dispatch.
    // We saturate the bus past capacity, then verify that a fresh subscriber
    // still receives an event afterwards.
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    // Burst-emit (well past the 1024 capacity).
    for i in 0..2000 {
        fire(
            &client,
            &shim.base,
            "fire_app_event",
            json!({"name": "burst", "payload": i}),
        )
        .await;
    }

    let (ready_tx, ready_rx) = oneshot::channel();
    let drain = tokio::spawn({
        let base = shim.base.clone();
        async move { drain_sse(&base, "main", 1, ready_tx).await }
    });
    ready_rx.await.unwrap();
    fire(
        &client,
        &shim.base,
        "fire_app_event",
        json!({"name": "after-burst", "payload": "ok"}),
    )
    .await;

    let records = drain.await.unwrap();
    assert!(records.iter().any(|r| r.event == "after-burst"));
    shim.shutdown().await;
}
