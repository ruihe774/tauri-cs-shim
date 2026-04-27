//! M2: `Result<T, E>` mapping and complex serde types.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::oneshot;

#[derive(Serialize, Deserialize)]
struct UserInput {
    first: String,
    last: String,
    age: u32,
}

#[derive(Serialize)]
struct UserView {
    full_name: String,
    age: u32,
}

#[derive(Serialize)]
struct StructuredErr {
    code: String,
    detail: String,
}

#[tauri::command]
fn divide(left: f64, right: f64) -> Result<f64, String> {
    if right == 0.0 {
        Err("divide by zero".into())
    } else {
        Ok(left / right)
    }
}

#[tauri::command]
fn make_user(input: UserInput) -> UserView {
    UserView {
        full_name: format!("{} {}", input.first, input.last),
        age: input.age,
    }
}

#[tauri::command]
async fn maybe_user(ok: bool) -> Result<UserView, StructuredErr> {
    if ok {
        Ok(UserView {
            full_name: "Ada Lovelace".into(),
            age: 36,
        })
    } else {
        Err(StructuredErr {
            code: "denied".into(),
            detail: "no user for you".into(),
        })
    }
}

#[tauri::command]
fn unit_ok() -> Result<(), String> {
    Ok(())
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
                divide, make_user, maybe_user, unit_ok
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
async fn result_ok_is_200_with_inner_value() {
    let shim = Shim::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/divide", shim.base))
        .json(&json!({"left": 10.0, "right": 4.0}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<serde_json::Value>().await.unwrap(), json!(2.5));
    shim.shutdown().await;
}

#[tokio::test]
async fn result_err_is_422_with_error_value() {
    let shim = Shim::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/divide", shim.base))
        .json(&json!({"left": 1.0, "right": 0.0}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap(),
        json!("divide by zero")
    );
    shim.shutdown().await;
}

#[tokio::test]
async fn complex_struct_round_trip() {
    let shim = Shim::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/make_user", shim.base))
        .json(&json!({"input": {"first": "Ada", "last": "Lovelace", "age": 36}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // serde defaults: snake_case fields are emitted snake_case.
    assert_eq!(body, json!({"full_name": "Ada Lovelace", "age": 36}));
    shim.shutdown().await;
}

#[tokio::test]
async fn structured_error_serializes_to_422_body() {
    let shim = Shim::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/maybe_user", shim.base))
        .json(&json!({"ok": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap(),
        json!({"code": "denied", "detail": "no user for you"})
    );
    shim.shutdown().await;
}

#[tokio::test]
async fn async_result_ok_works() {
    let shim = Shim::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/maybe_user", shim.base))
        .json(&json!({"ok": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["full_name"], "Ada Lovelace");
    shim.shutdown().await;
}

#[tokio::test]
async fn unit_ok_returns_null() {
    let shim = Shim::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/__tauri/invoke/unit_ok", shim.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<serde_json::Value>().await.unwrap(),
        serde_json::Value::Null
    );
    shim.shutdown().await;
}
