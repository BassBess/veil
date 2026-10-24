use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[tauri::command]
fn detect_pii(text: String) -> Result<serde_json::Value, String> {
    if text.len() > 100_000 {
        return Err("Text exceeds the current size limit".into());
    }

    // Locate the Python engine in our development repository.
    let engine_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../engines/python");

    #[cfg(windows)]
    let python = engine_dir.join(".venv/Scripts/python.exe");

    #[cfg(not(windows))]
    let python = engine_dir.join(".venv/bin/python");

    let request = serde_json::json!({ "text": text });

    let mut child = Command::new(python)
        .arg("-m")
        .arg("veil_engine.bridge")
        .current_dir(engine_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Unable to start the Python engine")?;

    {
        let mut stdin = child.stdin.take()
            .ok_or("Unable to communicate with Python")?;

        stdin
            .write_all(request.to_string().as_bytes())
            .map_err(|_| "Unable to send detection request")?;
    }

    let output = child.wait_with_output()
        .map_err(|_| "Python engine execution failed")?;

    if !output.status.success() {
        return Err("PII detection failed".into());
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|_| "Invalid response from Python engine".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![detect_pii])
        .run(tauri::generate_context!())
        .expect("error while building Veil");
}
