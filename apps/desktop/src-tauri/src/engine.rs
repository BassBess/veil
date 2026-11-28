//! Local engine IPC. Download workers never receive the detection request format.
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

const MAX_FRAME: u64 = 16_000_000;
const ENGINE_TIMEOUT: Duration = Duration::from_secs(180);

struct Worker {
    child: Child,
    stdin: ChildStdin,
    frames: mpsc::Receiver<Vec<u8>>,
}

impl Worker {
    fn start(mode: &str, root: &Path) -> Result<Self, String> {
        let mut child = crate::python_command("veil_engine.service")
            .args([mode.as_ref(), root.as_os_str()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Unable to start the local Python engine")?;
        let stdin = child.stdin.take().expect("piped engine stdin");
        let stdout = child.stdout.take().expect("piped engine stdout");
        let (send, frames) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut frame = Vec::new();
                match reader
                    .by_ref()
                    .take(MAX_FRAME)
                    .read_until(b'\n', &mut frame)
                {
                    Ok(size) if size > 0 && frame.ends_with(b"\n") => {
                        if send.send(frame).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            frames,
        })
    }

    fn send(&mut self, request: &Value) -> Result<(), String> {
        let mut frame = serde_json::to_vec(request).map_err(|_| "Invalid local engine request")?;
        frame.push(b'\n');
        self.stdin
            .write_all(&frame)
            .map_err(|_| "Unable to send the local engine request".into())
    }

    fn receive(&self, timeout: Duration) -> Result<Value, String> {
        let frame = self
            .frames
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => "The local engine operation timed out",
                mpsc::RecvTimeoutError::Disconnected => {
                    "The local engine stopped or returned an oversized response"
                }
            })?;
        serde_json::from_slice(&frame).map_err(|_| "Invalid response from the local engine".into())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone, Default)]
pub struct EngineState {
    worker: Arc<Mutex<Option<Worker>>>,
    detection: Arc<Mutex<Option<PathBuf>>>,
    download: Arc<Mutex<Option<Download>>>,
}

fn engine_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|_| "Unable to locate the local application folder")?
        .join("privacy-engine"))
}

fn payload(mut frame: Value) -> Result<Value, String> {
    if let Some(message) = frame.get("error").and_then(Value::as_str) {
        return Err(message.to_owned());
    }
    frame
        .as_object_mut()
        .and_then(|object| object.remove("result"))
        .ok_or_else(|| "Invalid response from the local engine".into())
}

impl EngineState {
    fn request(&self, root: &Path, request: Value) -> Result<Value, String> {
        let mut entry = self
            .worker
            .lock()
            .map_err(|_| "The local engine is unavailable")?;
        let worker = match entry.as_mut() {
            Some(worker) => worker,
            None => entry.insert(Worker::start("serve", root)?),
        };
        let result = worker
            .send(&request)
            .and_then(|_| worker.receive(ENGINE_TIMEOUT));
        if result.is_err() {
            *entry = None;
        }
        payload(result?)
    }

    pub(crate) fn detect(
        &self,
        root: &Path,
        text: String,
        entities: Vec<String>,
        rules: Vec<crate::protection::DetectionRule>,
        options: crate::protection::DetectionOptions,
    ) -> Result<Value, String> {
        crate::validate_detection_request(&text, &entities)?;
        options.validate()?;
        let directory =
            tempfile::tempdir().map_err(|_| "Unable to prepare detection cancellation")?;
        let cancel_path = directory.path().join("cancel");
        {
            let mut detection = self
                .detection
                .lock()
                .map_err(|_| "The local engine is unavailable")?;
            if detection.is_some() {
                return Err("Detection is already running".into());
            }
            *detection = Some(cancel_path.clone());
        }
        let result = self.request(root, json!({
            "action": "detect", "text": text, "entities": entities,
            "rules": rules, "language": options.language, "options": options, "cancel_path": cancel_path,
        }));
        let mut detection = self
            .detection
            .lock()
            .map_err(|_| "The local engine is unavailable")?;
        *detection = None;
        if cancel_path.exists() {
            return Err("Detection was cancelled.".into());
        }
        result
    }
}

pub(crate) async fn detect_local(
    app: tauri::AppHandle,
    state: EngineState,
    text: String,
    entities: Vec<String>,
    rules: Vec<crate::protection::DetectionRule>,
    options: crate::protection::DetectionOptions,
) -> Result<Value, String> {
    let root = engine_root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        state.detect(&root, text, entities, rules, options)
    })
    .await
    .map_err(|_| "The local detection task stopped unexpectedly".to_owned())?
}

#[tauri::command]
pub fn cancel_detection(state: State<'_, EngineState>) -> Result<(), String> {
    if let Some(path) = state
        .detection
        .lock()
        .map_err(|_| "The local engine is unavailable")?
        .as_ref()
    {
        std::fs::write(path, []).map_err(|_| "Unable to cancel detection")?;
    }
    Ok(())
}

#[tauri::command]
pub async fn engine_configuration(
    app: tauri::AppHandle,
    state: State<'_, EngineState>,
    action: String,
    id: Option<String>,
    profile: Option<Value>,
    mappings: Option<Value>,
    reset_token: Option<String>,
) -> Result<Value, String> {
    if !matches!(
        action.as_str(),
        "status"
            | "model_load"
            | "model_unload"
            | "model_remove"
            | "model_configuration"
            | "model_configure"
            | "model_label_reset_preview"
            | "model_label_reset"
            | "profile_save"
            | "profile_activate"
            | "profile_delete"
    ) {
        return Err("Unsupported engine configuration action".into());
    }
    let root = engine_root(&app)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut data = state.request(
            &root,
            json!({"action": action, "id": id, "profile": profile, "mappings": mappings,
                "reset_token": reset_token}),
        )?;
        if action == "status" {
            data["algorithms"]
                .as_array_mut()
                .ok_or("Invalid component registry")?
                .extend(crate::protection::MappingMode::components());
        }
        Ok(data)
    })
    .await
    .map_err(|_| "The engine configuration task stopped unexpectedly".to_owned())?
}

#[tauri::command]
pub async fn import_engine_file(
    app: tauri::AppHandle,
    state: State<'_, EngineState>,
    kind: String,
) -> Result<Option<Value>, String> {
    let root = engine_root(&app)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        let (selected, action) = match kind.as_str() {
            "model_file" => (
                dialog
                    .add_filter(
                        "Model files",
                        &["onnx", "gguf", "safetensors", "json", "veilmodel"],
                    )
                    .blocking_pick_file(),
                "model_import",
            ),
            "model_directory" => (dialog.blocking_pick_folder(), "model_import"),
            "profile" => (
                dialog
                    .add_filter("Veil profile", &["json"])
                    .blocking_pick_file(),
                "profile_import",
            ),
            _ => return Err("Choose a supported import type".into()),
        };
        let Some(file) = selected else {
            return Ok(None);
        };
        let path = file
            .into_path()
            .map_err(|_| "Choose a local file or folder")?;
        state
            .request(&root, json!({"action": action, "path": path}))
            .map(Some)
    })
    .await
    .map_err(|_| "The import task stopped unexpectedly".to_owned())?
}

#[tauri::command]
pub async fn export_engine_profile(
    app: tauri::AppHandle,
    state: State<'_, EngineState>,
    id: String,
) -> Result<bool, String> {
    let root = engine_root(&app)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profile = state.request(&root, json!({"action": "profile_export", "id": id}))?;
        let Some(file) = app
            .dialog()
            .file()
            .add_filter("Veil profile", &["json"])
            .set_file_name("veil-profile.json")
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = file.into_path().map_err(|_| "Choose a local file")?;
        let mut pending =
            tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid output folder")?)
                .map_err(|_| "Unable to prepare the profile file")?;
        serde_json::to_writer_pretty(&mut pending, &profile)
            .map_err(|_| "Unable to write the profile file")?;
        pending
            .as_file()
            .sync_all()
            .map_err(|_| "Unable to save the profile file")?;
        pending
            .persist(path)
            .map_err(|_| "Unable to replace the profile file")?;
        Ok(true)
    })
    .await
    .map_err(|_| "The profile export task stopped unexpectedly".to_owned())?
}

#[derive(Clone, Serialize)]
pub struct Download {
    id: String,
    status: &'static str,
    progress: Option<Value>,
    error: Option<String>,
    #[serde(skip)]
    cancel_path: PathBuf,
}

#[tauri::command]
pub fn model_download_status(state: State<'_, EngineState>) -> Result<Option<Download>, String> {
    Ok(state
        .download
        .lock()
        .map_err(|_| "The model download service is unavailable")?
        .clone())
}

#[tauri::command]
pub fn cancel_model_download(state: State<'_, EngineState>) -> Result<(), String> {
    let mut current = state
        .download
        .lock()
        .map_err(|_| "The model download service is unavailable")?;
    if let Some(job) = current.as_mut().filter(|job| job.status == "running") {
        std::fs::write(&job.cancel_path, []).map_err(|_| "Unable to cancel the model download")?;
        job.status = "cancelling";
    }
    Ok(())
}

#[tauri::command]
pub fn start_model_download(
    app: tauri::AppHandle,
    state: State<'_, EngineState>,
    source: Option<String>,
    model_id: Option<String>,
    authorized: bool,
) -> Result<Download, String> {
    let valid_source = source.as_deref().is_some_and(|source| !source.is_empty() && source.len() <= 640);
    let valid_model = model_id.as_deref().is_some_and(|id| {
        id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    });
    if !authorized || !((valid_source && model_id.is_none()) || (valid_model && source.is_none())) {
        return Err("Choose a Hugging Face model and authorize its download".into());
    }
    let root = engine_root(&app)?;
    let jobs = state.download.clone();
    let mut current = jobs
        .lock()
        .map_err(|_| "The model download service is unavailable")?;
    if current
        .as_ref()
        .is_some_and(|job| matches!(job.status, "running" | "cancelling"))
    {
        return Err("Wait for the current model download to finish".into());
    }
    let directory = tempfile::tempdir().map_err(|_| "Unable to prepare the download")?;
    let cancel_path = directory.path().join("cancel");
    let initial = Download {
        id: uuid::Uuid::new_v4().to_string(),
        status: "running",
        progress: None,
        error: None,
        cancel_path: cancel_path.clone(),
    };
    *current = Some(initial.clone());
    drop(current);
    tauri::async_runtime::spawn_blocking(move || {
        let request = json!({"source": source, "model_id": model_id, "authorized": authorized,
            "cancel_path": cancel_path});
        let result = run_download(&root, &request, &jobs);
        if let Ok(mut entry) = jobs.lock() {
            if let Some(job) = entry.as_mut() {
                match result {
                    Ok(_) => job.status = "finished",
                    Err(error) if error == "Model download cancelled." => job.status = "cancelled",
                    Err(error) => {
                        job.status = "failed";
                        job.error = Some(error);
                    }
                }
            }
        }
        drop(directory);
    });
    Ok(initial)
}

fn run_download(
    root: &Path,
    request: &Value,
    jobs: &Mutex<Option<Download>>,
) -> Result<Value, String> {
    let mut worker = Worker::start("download", root)?;
    worker.send(request)?;
    let deadline = Instant::now() + Duration::from_secs(1800);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let frame = worker.receive(remaining.min(Duration::from_secs(90)))?;
        if let Some(progress) = frame.get("progress") {
            if let Ok(mut entry) = jobs.lock() {
                if let Some(job) = entry.as_mut() {
                    job.progress = Some(progress.clone());
                }
            }
        } else {
            return payload(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the local Python engine and its base dependencies"]
    fn active_profile_is_used_through_ipc_and_survives_worker_restart() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let state = EngineState::default();
        let profile = state
            .request(
                root,
                json!({"action": "profile_save", "profile": {
                    "schema": 1, "name": "Fictional rule profile", "policy": "static",
                    "legacy_nlp": false, "languages": ["ar"], "budget": {},
                    "steps": [
                        {"component": "exact-span", "capability": "detection_fusion"},
                        {"component": "confidence-ranking", "capability": "entity_ranking"}
                    ]
                }}),
            )
            .unwrap();
        state
            .request(
                root,
                json!({"action": "profile_activate", "id": profile["id"]}),
            )
            .unwrap();
        drop(state);

        let reopened = EngineState::default();
        let status = reopened.request(root, json!({"action": "status"})).unwrap();
        assert_eq!(status["active_profile"], profile["id"]);
        let text = "🙂 بريد mira@example.com".to_string();
        let categories = vec!["EMAIL_ADDRESS".into()];
        let incompatible = reopened.detect(
            root,
            text.clone(),
            categories.clone(),
            vec![],
            Default::default(),
        );
        assert!(incompatible.unwrap_err().contains("does not support"));
        let options = crate::protection::DetectionOptions {
            language: "ar".into(),
            ..Default::default()
        };
        let result = reopened
            .detect(root, text, categories, vec![], options)
            .unwrap();
        assert_eq!(result["detections"][0]["text"], "mira@example.com");
        assert_eq!(result["detections"][0]["start"], 7);
        let status = reopened.request(root, json!({"action": "status"})).unwrap();
        assert_eq!(status["last_run"]["complete"], true);
        assert!(!status.to_string().contains("mira@example.com"));
        assert!(
            status["last_run"]["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|metric| metric["component"] == "confidence-ranking"
                    && metric["invocations"] == 1)
        );
    }
}
