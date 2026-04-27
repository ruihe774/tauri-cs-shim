//! End-to-end demo for the tauri-cs-shim. Exercises the surfaces a real
//! Tauri app uses: a sync command, an async command returning Result, a
//! command that mutates managed `State`, and a background task that emits
//! ticks through the SSE bus.

use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

#[derive(Default)]
struct Todos(Mutex<Vec<Todo>>);

#[derive(Clone, Serialize, Deserialize)]
struct Todo {
    id: u64,
    text: String,
}

#[derive(Deserialize)]
struct AddArgs {
    text: String,
}

#[tauri::command]
fn greet(name: String) -> String {
    format!("Hello, {name}! (from the Rust shim)")
}

#[tauri::command]
async fn slow_double(n: i64) -> Result<i64, String> {
    if n < 0 {
        return Err(format!("refuse to double {n}"));
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    Ok(n * 2)
}

#[tauri::command]
fn add_todo(args: AddArgs, state: State<'_, Todos>, app: AppHandle) -> Todo {
    let mut list = state.0.lock().unwrap();
    let id = list.last().map(|t| t.id + 1).unwrap_or(1);
    let todo = Todo { id, text: args.text };
    list.push(todo.clone());
    let _ = app.emit("todo-added", &todo);
    todo
}

#[tauri::command]
fn list_todos(state: State<'_, Todos>) -> Vec<Todo> {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
fn clear_todos(state: State<'_, Todos>, app: AppHandle) {
    state.0.lock().unwrap().clear();
    let _ = app.emit("todos-cleared", ());
}

fn main() {
    tauri::Builder::default()
        .manage(Todos::default())
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut n: u64 = 0;
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    n += 1;
                    let _ = handle.emit("tick", n);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            slow_double,
            add_todo,
            list_todos,
            clear_todos,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run shim");
}
