//! M4: macro-injected `AppHandle`, `Window`, and `WebviewWindow` parameters.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use tauri::Manager;
use tokio::sync::oneshot;

#[derive(Default)]
struct Recorded(Mutex<Vec<String>>);

#[tauri::command]
fn whoami(window: tauri::Window) -> String {
    window.label().to_string()
}

#[tauri::command]
fn whoami_webview(window: tauri::WebviewWindow) -> String {
    window.label().to_string()
}

#[tauri::command]
async fn touched_via_handle(
    app: tauri::AppHandle,
    state: tauri::State<'_, Recorded>,
    name: String,
) -> Result<bool, ()> {
    state.0.lock().unwrap().push(format!("touched: {name}"));
    // Bounce a second mutation through a spawned task using only the handle.
    let app2 = app.clone();
    let name2 = name.clone();
    tauri::async_runtime::spawn(async move {
        let recorded = app2.state::<Recorded>();
        recorded.0.lock().unwrap().push(format!("spawned: {name2}"));
    })
    .await
    .unwrap();
    Ok(true)
}

#[tauri::command]
fn list_windows(app: tauri::AppHandle) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().keys().cloned().collect();
    labels.sort();
    labels
}

#[tauri::command]
fn show_recorded(state: tauri::State<'_, Recorded>) -> Vec<String> {
    state.0.lock().unwrap().clone()
}

struct Shim {
    base: String,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<Result<(), tauri::Error>>>,
}

impl Shim {
    async fn spawn() -> Self {
        let server = tauri::Builder::default()
            .manage(Recorded::default())
            .invoke_handler(tauri::generate_handler![
                whoami,
                whoami_webview,
                touched_via_handle,
                list_windows,
                show_recorded
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
async fn window_label_defaults_to_main() {
    let shim = Shim::spawn().await;
    let body = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/whoami", shim.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(body, json!("main"));
    shim.shutdown().await;
}

#[tokio::test]
async fn window_label_from_header() {
    let shim = Shim::spawn().await;
    let body = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/whoami", shim.base))
        .header("X-Tauri-Window", "settings")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(body, json!("settings"));
    shim.shutdown().await;
}

#[tokio::test]
async fn webview_window_alias_works() {
    let shim = Shim::spawn().await;
    let body = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/whoami_webview", shim.base))
        .header("X-Tauri-Window", "popup")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(body, json!("popup"));
    shim.shutdown().await;
}

#[tokio::test]
async fn app_handle_works_in_spawned_task() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/invoke/touched_via_handle", shim.base))
        .json(&json!({"name": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let recorded: Vec<String> = client
        .post(format!("{}/__tauri/invoke/show_recorded", shim.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        recorded,
        vec!["touched: alice".to_string(), "spawned: alice".to_string()]
    );

    shim.shutdown().await;
}

#[tokio::test]
async fn webview_windows_registry_grows_with_clients() {
    let shim = Shim::spawn().await;
    let client = reqwest::Client::new();

    // Touching the server with a new window label registers it.
    let _ = client
        .post(format!("{}/__tauri/invoke/whoami", shim.base))
        .header("X-Tauri-Window", "popup")
        .json(&json!({}))
        .send()
        .await
        .unwrap();

    let labels: Vec<String> = client
        .post(format!("{}/__tauri/invoke/list_windows", shim.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(labels.contains(&"main".to_string()));
    assert!(labels.contains(&"popup".to_string()));

    shim.shutdown().await;
}

#[tokio::test]
async fn window_stub_methods_are_no_ops() {
    let server = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![whoami])
        .bind("127.0.0.1:0")
        .await
        .unwrap();
    let h = server.app_handle().clone();
    let w = h.get_webview_window("main").unwrap();
    assert_eq!(w.label(), "main");
    assert!(w.set_title("doesn't matter").is_ok());
    assert!(w.show().is_ok());
    assert!(w.hide().is_ok());
    assert!(w.close().is_ok());
    assert!(w.is_focused().unwrap());
    assert!(w.is_visible().unwrap());
    assert!(!w.is_minimized().unwrap());
}
