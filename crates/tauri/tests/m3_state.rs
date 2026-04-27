//! M3: managed state, `State<'_, T>` parameter injection, and the
//! `Manager` trait methods on `App` / `AppHandle`.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use tauri::Manager;
use tokio::sync::oneshot;

#[derive(Default)]
struct Counter(Mutex<i64>);

struct Greeting(String);

#[tauri::command]
fn bump(state: tauri::State<'_, Counter>, by: i64) -> i64 {
    let mut g = state.0.lock().unwrap();
    *g += by;
    *g
}

#[tauri::command]
fn read(state: tauri::State<'_, Counter>) -> i64 {
    *state.0.lock().unwrap()
}

#[tauri::command]
async fn greet_managed(state: tauri::State<'_, Greeting>, name: String) -> Result<String, ()> {
    Ok(format!("{}, {}!", state.0, name))
}

#[tauri::command]
fn missing_state(_state: tauri::State<'_, NotManaged>) -> i64 {
    0
}

struct NotManaged;

struct Shim {
    base: String,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<Result<(), tauri::Error>>>,
}

impl Shim {
    async fn spawn() -> Self {
        let server = tauri::Builder::default()
            .manage(Counter::default())
            .setup(|app| {
                app.manage(Greeting("Hi".into()));
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                bump,
                read,
                greet_managed,
                missing_state
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
async fn state_persists_across_invocations() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    let r1 = client
        .post(format!("{}/__tauri/invoke/bump", shim.base))
        .json(&json!({"by": 5}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(r1, json!(5));

    let r2 = client
        .post(format!("{}/__tauri/invoke/bump", shim.base))
        .json(&json!({"by": 3}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(r2, json!(8));

    let r3 = client
        .post(format!("{}/__tauri/invoke/read", shim.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(r3, json!(8));

    shim.shutdown().await;
}

#[tokio::test]
async fn setup_hook_can_register_state() {
    let shim = Shim::spawn().await;
    let body = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/greet_managed", shim.base))
        .json(&json!({"name": "world"}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(body, json!("Hi, world!"));
    shim.shutdown().await;
}

#[tokio::test]
async fn unmanaged_state_is_a_422() {
    let shim = Shim::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/missing_state", shim.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
    let body: serde_json::Value = resp.json().await.unwrap();
    let s = body.as_str().unwrap();
    assert!(s.contains("not managed"), "got: {s}");
    shim.shutdown().await;
}

#[tokio::test]
async fn manager_methods_round_trip() {
    // No HTTP needed — exercise the Manager trait directly.
    let server = tauri::Builder::default()
        .manage(Counter(Mutex::new(7)))
        .invoke_handler(tauri::generate_handler![bump])
        .bind("127.0.0.1:0")
        .await
        .unwrap();

    let h = server.app_handle().clone();
    assert_eq!(*h.state::<Counter>().0.lock().unwrap(), 7);
    assert!(h.try_state::<NotManaged>().is_none());

    // Re-managing the same type is a no-op (parity with upstream).
    assert!(!h.manage(Counter(Mutex::new(99))));
    assert_eq!(*h.state::<Counter>().0.lock().unwrap(), 7);
}

#[tokio::test]
async fn manage_a_new_type_after_construction() {
    let server = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![bump])
        .bind("127.0.0.1:0")
        .await
        .unwrap();
    let h = server.app_handle().clone();
    assert!(h.try_state::<Counter>().is_none());
    assert!(h.manage(Counter(Mutex::new(42))));
    assert_eq!(*h.state::<Counter>().0.lock().unwrap(), 42);
}
