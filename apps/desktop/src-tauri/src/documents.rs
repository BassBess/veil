use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};
use tauri_plugin_dialog::DialogExt;

const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct DocumentError {
    code: String,
    message: String,
}
impl DocumentError {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ParsedDocument {
    text: String,
    file_name: String,
    file_type: String,
    pages: Option<u32>,
    warnings: Vec<String>,
}

fn requested_file_limit(max_file_mb: Option<u64>) -> Result<u64, DocumentError> {
    let mb = max_file_mb.unwrap_or(100);
    if !(1..=100).contains(&mb) {
        return Err(DocumentError::new(
            "invalid_limit",
            "Choose an import limit between 1 and 100 MB.",
        ));
    }
    Ok((mb * 1024 * 1024).min(MAX_FILE_BYTES))
}

#[cfg(test)]
fn validate_file(path: &Path) -> Result<(), DocumentError> {
    validate_file_with_limit(path, MAX_FILE_BYTES)
}

fn validate_file_with_limit(path: &Path, max_file_bytes: u64) -> Result<(), DocumentError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "txt" | "docx" | "pdf" | "csv" | "xlsx") {
        return Err(DocumentError::new(
            "unsupported_type",
            "Choose a TXT, DOCX, PDF, CSV, or XLSX document.",
        ));
    }
    let file = File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => DocumentError::new(
            "missing_file",
            "The selected file no longer exists. Select it again.",
        ),
        _ => DocumentError::new(
            "unreadable_file",
            "Veil cannot read this file. Check its permissions or choose another copy.",
        ),
    })?;
    let metadata = file.metadata().map_err(|_| {
        DocumentError::new("unreadable_file", "Unable to read document information.")
    })?;
    if !metadata.is_file() {
        return Err(DocumentError::new(
            "invalid_file",
            "Select a regular document file.",
        ));
    }
    if metadata.len() > max_file_bytes {
        return Err(DocumentError::new(
            "file_too_large",
            &format!(
                "The file exceeds your {} MB import limit. Change it in Settings (maximum 100 MB).",
                max_file_bytes / (1024 * 1024)
            ),
        ));
    }
    if metadata.len() == 0 {
        return Err(DocumentError::new(
            "empty_file",
            "The selected file is empty.",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn parse_path(path: &Path) -> Result<ParsedDocument, DocumentError> {
    parse_path_with_limit(path, MAX_FILE_BYTES)
}

fn parse_path_with_limit(
    path: &Path,
    max_file_bytes: u64,
) -> Result<ParsedDocument, DocumentError> {
    validate_file_with_limit(path, max_file_bytes)?;
    let file_path = path.to_str().ok_or_else(|| {
        DocumentError::new(
            "invalid_path",
            "The document path is not valid Unicode. Rename the file and try again.",
        )
    })?;
    let mut command = super::engine_command();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            DocumentError::new(
                "engine_unavailable",
                "Unable to start the local Python parser. Check the engine environment.",
            )
        })?;
    let request = serde_json::json!({"action": "parse_document", "file_path": file_path, "max_file_bytes": max_file_bytes});
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| {
            DocumentError::new(
                "communication",
                "Unable to communicate with the local parser.",
            )
        })
        .and_then(|mut stdin| {
            stdin
                .write_all(request.to_string().as_bytes())
                .map_err(|_| {
                    DocumentError::new("communication", "Unable to send the local parsing request.")
                })
        });
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let stdout = child.stdout.take().expect("piped stdout");
    // Drain output concurrently so a large successful response cannot fill the pipe.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(4_000_001)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(if result.is_err() {
                    DocumentError::new("engine_failed", "The local parser stopped unexpectedly.")
                } else {
                    DocumentError::new(
                        "timeout",
                        "Parsing took too long. Try a smaller or simpler document.",
                    )
                });
            }
        }
    };
    let output = reader
        .join()
        .map_err(|_| DocumentError::new("engine_failed", "Unable to read the parser response."))?
        .map_err(|_| DocumentError::new("engine_failed", "Unable to read the parser response."))?;
    if !status?.success() {
        return Err(DocumentError::new(
            "engine_failed",
            "The local parser failed. Check Python dependencies or try another document.",
        ));
    }
    let response: serde_json::Value = serde_json::from_slice(&output).map_err(|_| {
        DocumentError::new(
            "invalid_response",
            "The local parser returned an invalid response.",
        )
    })?;
    if let Some(error) = response.get("error") {
        return Err(DocumentError::new(
            error["code"].as_str().unwrap_or("parse_failed"),
            error["message"]
                .as_str()
                .unwrap_or("The document could not be parsed."),
        ));
    }
    let document: ParsedDocument =
        serde_json::from_value(response["document"].clone()).map_err(|_| {
            DocumentError::new(
                "invalid_response",
                "The local parser returned incomplete document data.",
            )
        })?;
    if document.text.len() > 500_000 {
        return Err(DocumentError::new(
            "text_too_large",
            "Extracted text exceeds the 500,000-byte limit.",
        ));
    }
    Ok(document)
}

// Paths originate in the native picker, never arbitrary paths supplied by web content.
#[tauri::command]
pub async fn parse_document(
    app: tauri::AppHandle,
    max_file_mb: Option<u64>,
) -> Result<Option<ParsedDocument>, DocumentError> {
    let max_file_bytes = requested_file_limit(max_file_mb)?;
    tauri::async_runtime::spawn_blocking(move || {
        let selected = app
            .dialog()
            .file()
            .add_filter("Documents", &["txt", "docx", "pdf", "csv", "xlsx"])
            .blocking_pick_file();
        let Some(selected) = selected else {
            return Ok(None);
        };
        let path = selected
            .into_path()
            .map_err(|_| DocumentError::new("invalid_path", "Select a local document file."))?;
        parse_path_with_limit(&path, max_file_bytes).map(Some)
    })
    .await
    .map_err(|_| {
        DocumentError::new(
            "engine_failed",
            "The document import task stopped unexpectedly.",
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsupported_extension() {
        assert_eq!(
            validate_file(Path::new("report.exe")).unwrap_err().code,
            "unsupported_type"
        );
    }
    #[test]
    fn rejects_missing_file() {
        assert_eq!(
            validate_file(Path::new("/nonexistent-veil-document.txt"))
                .unwrap_err()
                .code,
            "missing_file"
        );
    }
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new(extension: &str, content: &[u8]) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "veil-{}-{nonce}-مستند.{extension}",
                std::process::id()
            ));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.write_all(content).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    #[test]
    fn rejects_empty_file() {
        let file = Fixture::new("txt", b"");
        assert_eq!(validate_file(&file.0).unwrap_err().code, "empty_file");
    }
    #[test]
    fn rejects_oversized_file() {
        let file = Fixture::new("txt", b"x");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file.0)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert_eq!(validate_file(&file.0).unwrap_err().code, "file_too_large");
    }
    #[test]
    fn accepts_uppercase_extension() {
        let file = Fixture::new("TXT", b"hello");
        assert!(validate_file(&file.0).is_ok());
    }
    #[test]
    #[ignore = "requires engines/python/.venv with requirements installed"]
    fn imported_text_reaches_existing_detection() {
        let file = Fixture::new("txt", b"Contact john@example.com");
        let document = parse_path(&file.0).unwrap();
        assert_eq!(document.file_type, "txt");
        assert!(document.file_name.contains("مستند"));
        let result = super::super::detect_pii(document.text, vec!["EMAIL_ADDRESS".into()]).unwrap();
        assert!(
            result["detections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["text"] == "john@example.com")
        );
    }
    #[test]
    #[ignore = "requires engines/python/.venv with requirements installed"]
    fn propagates_structured_parser_error() {
        let file = Fixture::new("docx", b"not a valid document");
        assert_eq!(parse_path(&file.0).unwrap_err().code, "invalid_file");
    }
}

#[derive(Default)]
pub struct DroppedDocuments(pub std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>);

impl DroppedDocuments {
    pub fn authorize(&self, paths: Vec<std::path::PathBuf>) {
        if let Ok(mut allowed) = self.0.lock() {
            *allowed = paths.into_iter().collect();
        }
    }

    fn consume(&self, path: &std::path::Path) -> Result<(), DocumentError> {
        let mut allowed = self
            .0
            .lock()
            .map_err(|_| DocumentError::new("drop_unavailable", "Unable to read dropped files"))?;
        if !allowed.remove(path) {
            return Err(DocumentError::new(
                "invalid_drop",
                "Drop the document into Veil again.",
            ));
        }
        allowed.clear();
        Ok(())
    }
}

#[tauri::command]
pub async fn parse_dropped_document(
    path: String,
    max_file_mb: Option<u64>,
    dropped: tauri::State<'_, DroppedDocuments>,
) -> Result<ParsedDocument, DocumentError> {
    let path = std::path::PathBuf::from(path);
    dropped.consume(&path)?;
    let max_file_bytes = requested_file_limit(max_file_mb)?;
    tauri::async_runtime::spawn_blocking(move || parse_path_with_limit(&path, max_file_bytes))
        .await
        .map_err(|_| {
            DocumentError::new(
                "engine_failed",
                "The document import task stopped unexpectedly",
            )
        })?
}

#[cfg(test)]
mod dropped_tests {
    use super::*;
    #[test]
    fn accepts_only_an_actual_native_drop_once() {
        let dropped = DroppedDocuments::default();
        let path = std::path::PathBuf::from("test.pdf");
        assert!(dropped.consume(&path).is_err());
        dropped.authorize(vec![path.clone()]);
        assert!(dropped.consume(&path).is_ok());
        assert!(dropped.consume(&path).is_err());
    }
}

#[cfg(test)]
mod import_limit_tests {
    use super::*;
    #[test]
    fn import_limit_defaults_and_bounds() {
        assert_eq!(requested_file_limit(None).unwrap(), MAX_FILE_BYTES);
        assert_eq!(requested_file_limit(Some(20)).unwrap(), 20 * 1024 * 1024);
        assert!(requested_file_limit(Some(0)).is_err());
        assert!(requested_file_limit(Some(101)).is_err());
        assert!(requested_file_limit(Some(u64::MAX)).is_err());
    }
    #[test]
    fn larger_document_requires_selected_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.pdf");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(21 * 1024 * 1024)
            .unwrap();
        assert_eq!(
            validate_file_with_limit(&path, requested_file_limit(Some(20)).unwrap())
                .unwrap_err()
                .code,
            "file_too_large"
        );
        assert!(validate_file_with_limit(&path, requested_file_limit(Some(50)).unwrap()).is_ok());
    }
}
