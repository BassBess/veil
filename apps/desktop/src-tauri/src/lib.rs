use tauri::Manager;
mod account_key;
mod documents;
#[cfg(test)]
mod identity_tests;
mod os_auth;
mod protection;
mod review;
mod vault_storage;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn validate_detection_request(text: &str, entities: &[String]) -> Result<(), String> {
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
        if !review::supported_entity(entity) {
            return Err(format!("Unsupported PII category: {entity}"));
        }
    }

    Ok(())
}

fn engine_command() -> Command {
    let engine_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../engines/python");

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
fn detect_pii(text: String, entities: Vec<String>) -> Result<serde_json::Value, String> {
    run_detection(text, entities, vec![])
}
fn run_detection(
    text: String,
    entities: Vec<String>,
    rules: Vec<protection::DetectionRule>,
) -> Result<serde_json::Value, String> {
    validate_detection_request(&text, &entities)?;

    let request = serde_json::json!({
        "text": text,
        "entities": entities,
        "rules": rules,
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

    serde_json::from_slice(&output.stdout).map_err(|_| "Invalid response from Python engine".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(protection::VaultState::default())
        .manage(documents::DroppedDocuments::default())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                window
                    .state::<documents::DroppedDocuments>()
                    .authorize(paths.clone());
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            detect_pii,
            protection::set_vault_protection,
            protection::save_detection_rules,
            protection::list_detection_rules,
            protection::detect_with_rules,
            protection::reset_vault,
            protection::vault_status,
            protection::unlock_vault,
            protection::lock_vault,
            protection::list_saved_identities,
            protection::save_identity,
            protection::find_saved_identity,
            protection::process_reviewed_text,
            protection::list_vault_records,
            protection::restore_vault_record,
            protection::clear_vault_records,
            protection::export_processed_text,
            documents::parse_document,
            documents::parse_dropped_document,
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
        assert!(validate_detection_request("John Smith", &categories()).is_ok());
    }

    #[test]
    fn rejects_empty_text() {
        assert!(validate_detection_request("   ", &categories()).is_err());
    }

    #[test]
    fn rejects_no_categories() {
        assert!(validate_detection_request("John Smith", &[]).is_err());
    }

    #[test]
    fn rejects_unknown_category() {
        assert!(validate_detection_request("John Smith", &["UNKNOWN".into()]).is_err());
    }

    #[test]
    fn rejects_oversized_input() {
        let text = "a".repeat(100_001);
        assert!(validate_detection_request(&text, &categories()).is_err());
    }
}

#[cfg(test)]
mod detection_review_flow_tests {
    use super::*;
    use crate::review::{ReviewedEntity, validate_reviewed_detections};

    #[test]
    #[ignore = "requires engines/python/.venv with the English model installed"]
    fn reviews_only_the_second_occurrence_of_a_detected_email() {
        let email = "mira@example.com";
        let text = format!("\u{1f600} Send to {email} or copy {email} for review.");
        let result = detect_pii(text.clone(), vec!["EMAIL_ADDRESS".into()]).unwrap();
        let hits = result["detections"].as_array().unwrap();
        assert_eq!(hits.len(), 2);

        for (hit, (byte_offset, _)) in hits.iter().zip(text.match_indices(email)) {
            let start = text[..byte_offset].chars().count();
            assert_eq!(hit["entity_type"], "EMAIL_ADDRESS");
            assert_eq!(hit["text"], email);
            assert_eq!(hit["start"], start);
            assert_eq!(hit["end"], start + email.len());
        }

        let mut accepted = hits[1].clone();
        accepted["id"] = "detection-1".into();
        let reviewed: ReviewedEntity = serde_json::from_value(accepted).unwrap();
        let approved = validate_reviewed_detections(text, vec![reviewed]).unwrap();
        assert_eq!(approved.len(), 1);
        assert_eq!(approved[0].id, "detection-1");
        assert_eq!(hits[1]["start"], approved[0].start);
    }
}
