//! M6: backend listeners + frontend->backend emit.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{Emitter, Listener, Manager};
use tokio::sync::oneshot;

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<(String, Value)>>>);

#[tauri::command]
fn show(state: tauri::State<'_, Recorded>) -> Vec<(String, Value)> {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
async fn emit_from_app(app: tauri::AppHandle, name: String) -> Result<(), String> {
    app.emit(&name, json!("from-rust")).map_err(|e| e.to_string())
}

#[tauri::command]
fn unlisten_id(state: tauri::State<'_, Mutex<Option<u32>>>, app: tauri::AppHandle) {
    if let Some(id) = state.lock().unwrap().take() {
        app.unlisten(id);
    }
}

struct Shim {
    base: String,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<Result<(), tauri::Error>>>,
}

impl Shim {
    async fn spawn() -> Self {
        let recorded = Recorded::default();
        let listener_id_state: Mutex<Option<u32>> = Mutex::new(None);
        let server = tauri::Builder::default()
            .manage(recorded.clone())
            .manage(listener_id_state)
            .setup({
                let recorded = recorded.clone();
                move |app| {
                    // Listen on App target. App listeners receive `Any`-target
                    // events (broadcast) and explicit `App`-target events.
                    let recorded2 = recorded.clone();
                    let id = app.listen("ping", move |ev| {
                        recorded2
                            .0
                            .lock()
                            .unwrap()
                            .push(("ping".into(), ev.payload));
                    });
                    // Stash the id so the `unlisten_id` command can remove it.
                    let h = app.handle();
                    if let Some(state) = h.try_state::<Mutex<Option<u32>>>() {
                        *state.lock().unwrap() = Some(id);
                    }

                    let recorded3 = recorded.clone();
                    app.listen_any("hello", move |ev| {
                        recorded3
                            .0
                            .lock()
                            .unwrap()
                            .push(("hello".into(), ev.payload));
                    });

                    let recorded4 = recorded.clone();
                    app.once("once-only", move |ev| {
                        recorded4
                            .0
                            .lock()
                            .unwrap()
                            .push(("once-only".into(), ev.payload));
                    });
                    Ok(())
                }
            })
            .invoke_handler(tauri::generate_handler![show, emit_from_app, unlisten_id])
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

async fn fetch_show(client: &reqwest::Client, base: &str) -> Vec<(String, Value)> {
    client
        .post(format!("{base}/__tauri/invoke/show"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn frontend_emit_reaches_backend_listener() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/emit", shim.base))
        .json(&json!({"event": "ping", "payload": "from-js"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Dispatch task is async; poll briefly.
    for _ in 0..30 {
        let recorded = fetch_show(&client, &shim.base).await;
        if !recorded.is_empty() {
            assert_eq!(recorded[0].0, "ping");
            assert_eq!(recorded[0].1, json!("from-js"));
            shim.shutdown().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("backend listener never fired");
}

#[tokio::test]
async fn backend_emit_reaches_backend_listener() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    let _ = client
        .post(format!("{}/__tauri/invoke/emit_from_app", shim.base))
        .json(&json!({"name": "ping"}))
        .send()
        .await
        .unwrap();

    for _ in 0..30 {
        let recorded = fetch_show(&client, &shim.base).await;
        if !recorded.is_empty() {
            assert_eq!(recorded[0].0, "ping");
            assert_eq!(recorded[0].1, json!("from-rust"));
            shim.shutdown().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("backend listener never fired for backend-emitted event");
}

#[tokio::test]
async fn once_listener_fires_exactly_once() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    for i in 0..3 {
        let _ = client
            .post(format!("{}/__tauri/emit", shim.base))
            .json(&json!({"event": "once-only", "payload": i}))
            .send()
            .await
            .unwrap();
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    let recorded = fetch_show(&client, &shim.base).await;
    let once_hits: Vec<_> = recorded
        .iter()
        .filter(|(name, _)| name == "once-only")
        .collect();
    assert_eq!(
        once_hits.len(),
        1,
        "expected exactly one delivery, got {}",
        once_hits.len()
    );
    shim.shutdown().await;
}

#[tokio::test]
async fn listen_any_picks_up_labelled_emit() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    // emit with target = AnyLabel{popup} — the upstream quirk says an `Any`
    // listener does NOT receive this, but `listen_any` is also Any; verify
    // we replicate that.
    let _ = client
        .post(format!("{}/__tauri/emit", shim.base))
        .json(&json!({
            "event": "hello",
            "payload": "labelled",
            "target": {"kind": "AnyLabel", "label": "popup"}
        }))
        .send()
        .await
        .unwrap();

    // And another emit with target = Any — listen_any must catch this.
    let _ = client
        .post(format!("{}/__tauri/emit", shim.base))
        .json(&json!({"event": "hello", "payload": "broadcast"}))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    let recorded = fetch_show(&client, &shim.base).await;
    let hello: Vec<_> = recorded
        .iter()
        .filter(|(name, _)| name == "hello")
        .collect();
    assert_eq!(
        hello.len(),
        1,
        "listen_any should receive the Any-target event but not the AnyLabel one (upstream quirk)"
    );
    assert_eq!(hello[0].1, json!("broadcast"));

    shim.shutdown().await;
}

#[tokio::test]
async fn unlisten_removes_the_handler() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    // First fire goes through.
    let _ = client
        .post(format!("{}/__tauri/emit", shim.base))
        .json(&json!({"event": "ping", "payload": "first"}))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let recorded = fetch_show(&client, &shim.base).await;
    assert!(recorded.iter().any(|(n, _)| n == "ping"));

    // Unregister the listener.
    let _ = client
        .post(format!("{}/__tauri/invoke/unlisten_id", shim.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();

    // Second fire should not reach the recorder.
    let _ = client
        .post(format!("{}/__tauri/emit", shim.base))
        .json(&json!({"event": "ping", "payload": "second"}))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let recorded = fetch_show(&client, &shim.base).await;
    let ping_count = recorded.iter().filter(|(n, _)| n == "ping").count();
    assert_eq!(ping_count, 1, "post-unlisten event should not be recorded");
    shim.shutdown().await;
}

#[tokio::test]
async fn invalid_emit_body_is_a_400() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/emit", shim.base))
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    shim.shutdown().await;
}
