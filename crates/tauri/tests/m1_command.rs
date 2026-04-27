//! M1 integration test: a sync `#[command]` round-trips through HTTP.

use std::time::Duration;

use serde_json::json;
use tokio::sync::oneshot;

#[tauri::command]
fn greet(name: String) -> String {
    format!("Hello, {name}!")
}

#[tauri::command]
fn add(left: i64, right: i64) -> i64 {
    left + right
}

#[tauri::command]
async fn shout(message: String) -> String {
    message.to_uppercase()
}

struct ShimHandle {
    base_url: String,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<Result<(), tauri::Error>>>,
}

impl ShimHandle {
    async fn spawn() -> Self {
        let server = tauri::Builder::default()
            .invoke_handler(tauri::generate_handler![greet, add, shout])
            .bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = server.local_addr();
        let (tx, rx) = oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            server
                .serve_with_shutdown(async move {
                    let _ = rx.await;
                })
                .await
        });
        Self {
            base_url: format!("http://{addr}"),
            shutdown: Some(tx),
            join: Some(join),
        }
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
        }
    }
}

#[tokio::test]
async fn greet_round_trip() {
    let shim = ShimHandle::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/invoke/greet", shim.base_url))
        .json(&json!({ "name": "world" }))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body, json!("Hello, world!"));

    shim.shutdown().await;
}

#[tokio::test]
async fn add_takes_multiple_args() {
    let shim = ShimHandle::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/invoke/add", shim.base_url))
        .json(&json!({ "left": 2, "right": 3 }))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(resp.json::<serde_json::Value>().await.unwrap(), json!(5));

    shim.shutdown().await;
}

#[tokio::test]
async fn async_command_works() {
    let shim = ShimHandle::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/invoke/shout", shim.base_url))
        .json(&json!({ "message": "hello" }))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap(),
        json!("HELLO")
    );

    shim.shutdown().await;
}

#[tokio::test]
async fn missing_argument_is_a_422() {
    let shim = ShimHandle::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/invoke/greet", shim.base_url))
        .json(&json!({}))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = resp.json().await.expect("json");
    let s = body.as_str().expect("error body should be a string");
    assert!(s.contains("missing argument"), "got: {s}");
    assert!(s.contains("name"), "got: {s}");

    shim.shutdown().await;
}

#[tokio::test]
async fn unknown_command_is_a_422() {
    let shim = ShimHandle::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/invoke/nope", shim.base_url))
        .json(&json!({}))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["kind"], "not_found");
    assert_eq!(body["command"], "nope");

    shim.shutdown().await;
}

#[tokio::test]
async fn invalid_json_body_is_a_400() {
    let shim = ShimHandle::spawn().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/__tauri/invoke/greet", shim.base_url))
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    shim.shutdown().await;
}
