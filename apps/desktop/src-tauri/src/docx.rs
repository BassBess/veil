use base64::Engine;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri_plugin_dialog::DialogExt;
use zeroize::Zeroizing;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Replacement {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub replacement: String,
}

pub(crate) struct Source {
    id: String,
    bytes: Zeroizing<Vec<u8>>,
    text: Zeroizing<String>,
}

#[derive(Clone, Default)]
pub struct DocxState(Arc<Mutex<Option<Arc<Source>>>>);

impl DocxState {
    pub(crate) fn remember(&self, bytes: Vec<u8>, text: String) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let source = Source {
            id: id.clone(),
            bytes: Zeroizing::new(bytes),
            text: Zeroizing::new(text),
        };
        *self.0.lock().map_err(|_| "The imported document is unavailable")? = Some(Arc::new(source));
        Ok(id)
    }

    pub(crate) fn clear(&self, id: Option<&str>) -> Result<(), String> {
        let mut current = self.0.lock().map_err(|_| "The imported document is unavailable")?;
        if id.is_none() || current.as_ref().is_some_and(|source| Some(source.id.as_str()) == id) {
            *current = None;
        }
        Ok(())
    }

    fn source(&self, id: &str) -> Result<Arc<Source>, String> {
        self.0.lock().map_err(|_| "The imported document is unavailable")?
            .as_ref().filter(|source| source.id == id).cloned()
            .ok_or_else(|| "Import and process the Word document again before exporting.".into())
    }
}

#[tauri::command]
pub fn clear_docx_source(source_id: String, state: tauri::State<'_, DocxState>) -> Result<(), String> {
    state.clear(Some(&source_id))
}

fn validate_output(original: &str, output: &str, replacements: &[Replacement]) -> Result<(), String> {
    if original.len() > 500_000 || output.len() > 5_000_000 || replacements.is_empty() {
        return Err("Process the imported Word document before exporting.".into());
    }
    let chars: Vec<char> = original.chars().collect();
    let mut expected = String::new();
    let mut cursor = 0;
    for item in replacements {
        if item.start < cursor || item.start >= item.end || item.end > chars.len()
            || item.replacement.is_empty() || item.replacement.len() > 1000
            || item.replacement.chars().any(char::is_control)
            || chars[item.start..item.end].iter().collect::<String>() != item.text {
            return Err("DOCX replacements no longer match the document. Process it again.".into());
        }
        expected.extend(chars[cursor..item.start].iter());
        expected.push_str(&item.replacement);
        cursor = item.end;
    }
    expected.extend(chars[cursor..].iter());
    if expected != output {
        return Err("The DOCX replacements do not match the processed preview. Process it again.".into());
    }
    Ok(())
}

fn render_docx(source: &Source, replacements: Vec<Replacement>) -> Result<Vec<u8>, String> {
    let request = serde_json::json!({
        "action": "export_docx",
        "source_docx": base64::engine::general_purpose::STANDARD.encode(&*source.bytes),
        "text": &*source.text,
        "replacements": replacements,
    });
    let mut child = crate::engine_command()
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|_| "Unable to start the local Word exporter")?;
    let stdout = child.stdout.take().ok_or("Unable to read Word export output")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(180_000_001).read_to_end(&mut bytes).map(|_| bytes)
    });
    let sent = child.stdin.take().ok_or("Unable to communicate with the Word exporter")
        .and_then(|mut stdin| stdin.write_all(request.to_string().as_bytes())
            .map_err(|_| "Unable to send the Word export request"));
    if let Err(error) = sent {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return Err(error.into());
    }
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("Word export stopped or exceeded its time limit. Try a smaller document.");
            }
        }
    };
    let bytes = reader.join().map_err(|_| "Unable to read Word export output")?
        .map_err(|_| "Unable to read Word export output")?;
    if !status?.success() || bytes.len() > 180_000_000 {
        return Err("The Word document could not be exported.".into());
    }
    let response: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid response from the Word exporter")?;
    if let Some(error) = response.get("error") {
        return Err(error["message"].as_str().unwrap_or("Word export failed").into());
    }
    let docx = base64::engine::general_purpose::STANDARD.decode(
        response["docx"].as_str().ok_or("Missing Word export output")?
    ).map_err(|_| "Invalid Word export output")?;
    if !docx.starts_with(b"PK\x03\x04") {
        return Err("The exporter did not return a Word document.".into());
    }
    Ok(docx)
}

#[tauri::command]
pub async fn export_processed_docx(
    app: tauri::AppHandle,
    source_id: String,
    original_text: String,
    processed_text: String,
    replacements: Vec<Replacement>,
    state: tauri::State<'_, DocxState>,
) -> Result<bool, String> {
    let source = state.source(&source_id)?;
    if *source.text != original_text {
        return Err("The edited text no longer matches the imported Word document. Import and process it again.".into());
    }
    validate_output(&source.text, &processed_text, &replacements)?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app.dialog().file().add_filter("Word document", &["docx"])
            .set_file_name("veil-processed.docx").blocking_save_file() else {
                return Ok(false);
            };
        let path = file.into_path().map_err(|_| "Choose a local output file")?.with_extension("docx");
        let bytes = render_docx(&source, replacements)?;
        let mut pending = tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid output folder")?)
            .map_err(|_| "Unable to create the Word output file")?;
        pending.write_all(&bytes).and_then(|_| pending.as_file().sync_all())
            .map_err(|_| "Unable to save the Word document")?;
        pending.persist(path).map_err(|_| "Unable to replace the Word output file")?;
        Ok(true)
    }).await.map_err(|_| "The Word export task stopped unexpectedly".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement() -> Replacement {
        Replacement { start: 2, end: 6, text: "John".into(), replacement: "PERSON_001".into() }
    }

    #[test]
    fn output_matches_exact_unicode_occurrences() {
        assert!(validate_output("😀 John met John", "😀 PERSON_001 met John", &[replacement()]).is_ok());
        assert!(validate_output("😀 John met John", "😀 PERSON_001 met PERSON_001", &[replacement()]).is_err());
    }

    #[test]
    fn rejects_stale_overlapping_and_invalid_replacements() {
        let item = replacement();
        assert!(validate_output("😀 Jane", "😀 PERSON_001", &[item.clone()]).is_err());
        assert!(validate_output("😀 John", "😀 PERSON_001", &[item.clone(), item.clone()]).is_err());
        assert!(validate_output("😀 John", "😀 PERSON_001", &[Replacement { end: 100, ..item }]).is_err());
    }

    #[test]
    fn clearing_old_source_cannot_clear_new_import() {
        let state = DocxState::default();
        let old = state.remember(vec![1], "old".into()).unwrap();
        let new = state.remember(vec![2], "new".into()).unwrap();
        state.clear(Some(&old)).unwrap();
        assert!(state.source(&old).is_err());
        assert_eq!(&*state.source(&new).unwrap().text, "new");
        state.clear(Some(&new)).unwrap();
        assert!(state.source(&new).is_err());
    }

    #[test]
    #[ignore = "requires the local Python DOCX dependency"]
    fn original_snapshot_exports_after_source_file_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let original_path = directory.path().join("original.docx");
        let output_path = directory.path().join("protected.docx");
        let engine = crate::engine_command();
        let create = std::process::Command::new(engine.get_program())
            .env("PYTHONUTF8", "1")
            .args(["-c", r#"
from docx import Document
from docx.shared import Pt
import sys
document = Document()
document.sections[0].header.paragraphs[0].text = 'Confidential'
paragraph = document.add_paragraph()
run = paragraph.add_run('John ')
run.bold = True
run.font.name = 'Arial'
run.font.size = Pt(14)
paragraph.add_run('Smith').italic = True
document.sections[0].footer.paragraphs[0].text = 'Footer'
document.save(sys.argv[1])
"#]).arg(&original_path).output().unwrap();
        assert!(create.status.success(), "{}", String::from_utf8_lossy(&create.stderr));
        let state = DocxState::default();
        let imported = crate::documents::import_path_with_limit(&original_path, 20 * 1024 * 1024, &state).unwrap();
        std::fs::remove_file(&original_path).unwrap();
        let start = imported.text.find("John Smith").unwrap();
        let mut vault = crate::protection::SessionVault::default();
        let record = vault.process(imported.text.clone(), vec![crate::review::ReviewedEntity {
            id: "selected-person".into(), entity_type: "PERSON".into(), text: "John Smith".into(),
            start, end: start + 10,
        }], "anonymize").unwrap();
        validate_output(&imported.text, &record.text, &record.export_replacements).unwrap();
        // Replacement metadata is transient and does not change saved record format.
        assert!(serde_json::to_value(&record).unwrap().get("export_replacements").is_none());
        let source = state.source(imported.source_id.as_deref().unwrap()).unwrap();
        std::fs::write(&output_path, render_docx(&source, record.export_replacements).unwrap()).unwrap();
        let check = std::process::Command::new(engine.get_program())
            .env("PYTHONUTF8", "1")
            .args(["-c", r#"
from docx import Document
from docx.shared import Pt
import sys
document = Document(sys.argv[1])
assert document.paragraphs[0].text == 'PERSON_001'
run = document.paragraphs[0].runs[0]
assert run.bold and run.font.name == 'Arial' and run.font.size == Pt(14)
assert document.sections[0].header.paragraphs[0].text == 'Confidential'
assert document.sections[0].footer.paragraphs[0].text == 'Footer'
"#]).arg(&output_path).output().unwrap();
        assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
    }

}
