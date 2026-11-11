mod review;
mod documents;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn validate_detection_request(
    text: &str,
    entities: &[String],
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Please enter text before detection".into());
    }

    if text.len() > 100_000 {
        return Err("Text exceeds the 100,000-byte limit".into());
    }

    if entities.is_empty() {
        return Err("Select at least one PII category".into());
    }

    for entity in entities {
        if !matches!(
            entity.as_str(),
            "PERSON"
                | "EMAIL_ADDRESS"
                | "PHONE_NUMBER"
                | "LOCATION"
                | "ORGANIZATION"
        ) {
            return Err(format!("Unsupported PII category: {entity}"));
        }
    }

    Ok(())
}

fn engine_command() -> Command {
    let engine_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../engines/python");

    #[cfg(windows)]
    let python = engine_dir.join(".venv/Scripts/python.exe");

    #[cfg(not(windows))]
    let python = engine_dir.join(".venv/bin/python");

    let mut command = Command::new(python);
    command
        .env("PYTHONUTF8", "1")
        .arg("-m")
        .arg("veil_engine.bridge")
        .current_dir(engine_dir);
    command
}

#[tauri::command]
fn detect_pii(
    text: String,
    entities: Vec<String>,
) -> Result<serde_json::Value, String> {
    validate_detection_request(&text, &entities)?;

    let request = serde_json::json!({
        "text": text,
        "entities": entities,
    });

    let mut child = engine_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Unable to start the Python engine")?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or("Unable to communicate with Python")?;

        stdin
            .write_all(request.to_string().as_bytes())
            .map_err(|_| "Unable to send detection request")?;
    }

    let output = child
        .wait_with_output()
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
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            detect_pii,
            documents::parse_document,
            review::validate_reviewed_detections
        ])
        .run(tauri::generate_context!())
        .expect("error while building Veil");
}

#[cfg(test)]
mod detection_request_tests {
    use super::*;

    fn categories() -> Vec<String> {
        vec!["PERSON".into()]
    }

    #[test]
    fn accepts_valid_request() {
        assert!(validate_detection_request(
            "John Smith",
            &categories()
        ).is_ok());
    }

    #[test]
    fn rejects_empty_text() {
        assert!(validate_detection_request(
            "   ",
            &categories()
        ).is_err());
    }

    #[test]
    fn rejects_no_categories() {
        assert!(validate_detection_request(
            "John Smith",
            &[]
        ).is_err());
    }

    #[test]
    fn rejects_unknown_category() {
        assert!(validate_detection_request(
            "John Smith",
            &["UNKNOWN".into()]
        ).is_err());
    }

    #[test]
    fn rejects_oversized_input() {
        let text = "a".repeat(100_001);
        assert!(validate_detection_request(
            &text,
            &categories()
        ).is_err());
    }
}
