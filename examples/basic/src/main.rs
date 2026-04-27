//! Minimal Tauri-style app exercising M1 of the debug shim:
//! a single sync command that round-trips through HTTP.

#[tauri::command]
fn greet(name: String) -> String {
    format!("Hello, {name}!")
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("failed to run shim");
}
