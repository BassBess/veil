use crate::review::{ReviewedEntity, validate_reviewed_detections};
use crate::vault_storage::UnlockedVault;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize)]
pub struct ProcessedRecord {
    pub(crate) id: String,
    mode: String,
    pub(crate) text: String,
    count: usize,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct StoredRecord {
    public: ProcessedRecord,
    original: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedIdentity {
    pub(crate) id: String,
    entity_type: String,
    original: String,
    pub(crate) pseudonym: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SessionVault {
    pub(crate) records: Vec<StoredRecord>,
    pub(crate) pseudonyms: Vec<SavedIdentity>,
    #[serde(default)]
    counters: HashMap<String, usize>,
    #[serde(default)]
    pub(crate) rules: Vec<DetectionRule>,
}

#[derive(Default)]
pub struct VaultState(
    pub Mutex<Option<UnlockedVault>>,
    pub std::sync::atomic::AtomicBool,
);

fn identity_key(category: &str, value: &str) -> String {
    match category {
        "PERSON" | "LOCATION" | "ORGANIZATION" => value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase(),
        _ => value.trim().to_string(),
    }
}

impl SessionVault {
    pub(crate) fn save_identity(
        &mut self,
        category: &str,
        original: &str,
        source: &str,
    ) -> Result<SavedIdentity, String> {
        let prefix = match category {
            "PERSON" => "PERSON",
            "EMAIL_ADDRESS" => "EMAIL",
            "PHONE_NUMBER" => "PHONE",
            "LOCATION" => "LOCATION",
            "ORGANIZATION" => "ORG",
            "STREET_ADDRESS" => "ADDRESS",
            "POSTAL_CODE" => "POSTCODE",
            "DATE_TIME" => "DATE",
            "CREDIT_CARD" => "CARD",
            "IBAN_CODE" => "IBAN",
            "IP_ADDRESS" => "IP",
            "URL" => "URL",
            "ID_NUMBER" => "ID",
            "INDIRECT_IDENTIFIER" => "DETAIL",
            _ => return Err("Unsupported identity category".into()),
        };
        let original = original.trim();
        if original.is_empty() || original.len() > 10_000 {
            return Err("Enter an identity between 1 and 10,000 bytes".into());
        }
        let key = identity_key(category, original);
        if let Some(identity) = self.pseudonyms.iter().find(|identity| {
            identity.entity_type == category && identity_key(category, &identity.original) == key
        }) {
            return Ok(identity.clone());
        }
        let counter = self.counters.entry(category.into()).or_default();
        let pseudonym = loop {
            *counter += 1;
            let candidate = format!("{prefix}_{:03}", *counter);
            if !source.contains(&candidate) {
                break candidate;
            }
        };
        let identity = SavedIdentity {
            id: uuid::Uuid::new_v4().to_string(),
            entity_type: category.into(),
            original: original.into(),
            pseudonym,
        };
        self.pseudonyms.push(identity.clone());
        Ok(identity)
    }

    pub(crate) fn process(
        &mut self,
        text: String,
        detections: Vec<ReviewedEntity>,
        mode: &str,
    ) -> Result<ProcessedRecord, String> {
        if !matches!(mode, "anonymize" | "pseudonymize") {
            return Err("Choose a valid processing mode".into());
        }
        let approved = validate_reviewed_detections(text.clone(), detections)?;
        if approved.is_empty() {
            return Err("Accept at least one detection before processing".into());
        }
        if self.records.len() >= 100 {
            return Err("Document history is full. Clear history in the Vault; saved identities will be kept.".into());
        }
        let chars: Vec<char> = text.chars().collect();
        let mut output = String::new();
        let mut cursor = 0;
        for entity in &approved {
            output.extend(chars[cursor..entity.start].iter());
            if mode == "anonymize" {
                output.push_str("[REDACTED]");
            } else {
                let identity = self.save_identity(&entity.entity_type, &entity.text, &text)?;
                output.push_str(&identity.pseudonym);
            }
            cursor = entity.end;
        }
        output.extend(chars[cursor..].iter());
        let record = ProcessedRecord {
            id: uuid::Uuid::new_v4().to_string(),
            mode: mode.into(),
            text: output,
            count: approved.len(),
        };
        self.records.push(StoredRecord {
            public: record.clone(),
            original: (mode == "pseudonymize").then_some(text),
        });
        Ok(record)
    }

    pub(crate) fn restore(&self, id: &str) -> Result<String, String> {
        self.records
            .iter()
            .find(|record| record.public.id == id)
            .ok_or("The record is no longer in the vault")?
            .original
            .clone()
            .ok_or_else(|| "Anonymized records cannot be restored".into())
    }
}

fn vault_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|_| "Unable to locate the local vault folder")?
        .join("identities.vault"))
}

#[derive(Serialize)]
pub struct VaultStatus {
    exists: bool,
    unlocked: bool,
    account_supported: bool,
    authentication_required: bool,
    needs_migration: bool,
    code_required: bool,
}

#[tauri::command]
pub fn vault_status(
    app: tauri::AppHandle,
    vault: State<'_, VaultState>,
) -> Result<VaultStatus, String> {
    let path = vault_path(&app)?;
    let exists = path.exists();
    let authentication_required =
        exists && UnlockedVault::requires_authentication(&path).unwrap_or(true);
    // A damaged file remains resettable, but is never treated as a new Vault.
    let needs_migration = cfg!(any(windows, target_os = "macos"))
        && exists
        && UnlockedVault::needs_passphrase_migration(&path).unwrap_or(false);
    if !vault.1.swap(true, std::sync::atomic::Ordering::SeqCst)
        && exists
        && !needs_migration
        && !authentication_required
        && cfg!(any(windows, target_os = "macos"))
    {
        let mut opened = vault.0.lock().map_err(|_| "The Vault is unavailable")?;
        if opened.is_none() {
            *opened = Some(UnlockedVault::open_account(path.clone(), false)?);
        }
    }
    Ok(VaultStatus {
        exists,
        authentication_required,
        needs_migration,
        code_required: exists && UnlockedVault::needs_passphrase_migration(&path).unwrap_or(false),
        account_supported: cfg!(any(windows, target_os = "macos")),
        unlocked: vault
            .0
            .lock()
            .map_err(|_| "The vault is unavailable")?
            .is_some(),
    })
}

#[tauri::command]
pub async fn unlock_vault(
    app: tauri::AppHandle,
    window: tauri::Window,
    password: String,
    create: bool,
    use_account: Option<bool>,
    require_authentication: Option<bool>,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    let password = Zeroizing::new(password);
    let path = vault_path(&app)?;
    let require_authentication = if path.exists() {
        UnlockedVault::requires_authentication(&path)?
    } else {
        require_authentication.unwrap_or(false)
    };
    if require_authentication {
        crate::os_auth::verify(window).await?;
    }
    let mut state = vault.0.lock().map_err(|_| "The vault is unavailable")?;
    if state.is_some() {
        return Err("The vault is already unlocked".into());
    }
    let use_account = use_account.unwrap_or(cfg!(any(windows, target_os = "macos")));
    if use_account && password.is_empty() {
        *state = Some(UnlockedVault::open_account_with_auth(
            path,
            false,
            require_authentication,
        )?);
    } else {
        let opened = UnlockedVault::open(path, &password, create)?;
        *state = Some(if use_account {
            opened.migrate_to_account()?
        } else {
            opened
        });
    }
    Ok(())
}

fn check_reset_confirmation(confirmation: &str) -> Result<(), String> {
    if confirmation != "RESET VAULT" {
        return Err("Type RESET VAULT to confirm permanently resetting the Vault".into());
    }
    Ok(())
}

#[tauri::command]
pub fn reset_vault(
    app: tauri::AppHandle,
    confirmation: String,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    check_reset_confirmation(&confirmation)?;
    let mut state = vault.0.lock().map_err(|_| "The vault is unavailable")?;
    if let Some(opened) = state.as_mut() {
        opened.update(|data| {
            *data = SessionVault::default();
            Ok(())
        })?;
    } else if cfg!(any(windows, target_os = "macos")) {
        *state = Some(UnlockedVault::open_account(vault_path(&app)?, true)?);
    } else {
        return Err("Unlock your Vault before resetting it".into());
    }
    Ok(())
}

#[tauri::command]
pub fn lock_vault(vault: State<'_, VaultState>) -> Result<(), String> {
    *vault.0.lock().map_err(|_| "The vault is unavailable")? = None;
    Ok(())
}

#[tauri::command]
pub fn process_reviewed_text(
    text: String,
    detections: Vec<ReviewedEntity>,
    mode: String,
    vault: State<'_, VaultState>,
) -> Result<ProcessedRecord, String> {
    vault
        .0
        .lock()
        .map_err(|_| "The vault is unavailable")?
        .as_mut()
        .ok_or("Unlock the Vault before processing")?
        .update(|data| data.process(text, detections, &mode))
}

#[tauri::command]
pub fn list_vault_records(vault: State<'_, VaultState>) -> Result<Vec<ProcessedRecord>, String> {
    Ok(vault
        .0
        .lock()
        .map_err(|_| "The vault is unavailable")?
        .as_ref()
        .ok_or("Unlock the Vault to view records")?
        .data
        .records
        .iter()
        .rev()
        .map(|record| record.public.clone())
        .collect())
}

#[tauri::command]
pub fn list_saved_identities(vault: State<'_, VaultState>) -> Result<Vec<SavedIdentity>, String> {
    Ok(vault
        .0
        .lock()
        .map_err(|_| "The vault is unavailable")?
        .as_ref()
        .ok_or("Unlock the Vault to view identities")?
        .data
        .pseudonyms
        .clone())
}

#[tauri::command]
pub fn save_identity(
    original: String,
    entity_type: String,
    vault: State<'_, VaultState>,
) -> Result<SavedIdentity, String> {
    vault
        .0
        .lock()
        .map_err(|_| "The vault is unavailable")?
        .as_mut()
        .ok_or("Unlock the Vault before saving identities")?
        .update(|data| data.save_identity(&entity_type, &original, ""))
}

pub(crate) fn find_matches(
    identity: &SavedIdentity,
    text: &str,
) -> Result<Vec<ReviewedEntity>, String> {
    if text.len() > 500_000 {
        return Err("Text exceeds the 500,000-byte limit".into());
    }
    let pattern = regex::RegexBuilder::new(&regex::escape(&identity.original))
        .case_insensitive(matches!(
            identity.entity_type.as_str(),
            "PERSON" | "ORGANIZATION" | "LOCATION"
        ))
        .build()
        .map_err(|_| "Unable to match the saved identity")?;
    let mut found = Vec::new();
    for hit in pattern.find_iter(text) {
        let before = text[..hit.start()].chars().next_back();
        let after = text[hit.end()..].chars().next();
        if before.is_some_and(char::is_alphanumeric) || after.is_some_and(char::is_alphanumeric) {
            continue;
        }
        found.push(ReviewedEntity {
            id: uuid::Uuid::new_v4().to_string(),
            entity_type: identity.entity_type.clone(),
            text: hit.as_str().into(),
            start: text[..hit.start()].chars().count(),
            end: text[..hit.end()].chars().count(),
        });
    }
    Ok(found)
}

#[tauri::command]
pub fn find_saved_identity(
    id: String,
    text: String,
    vault: State<'_, VaultState>,
) -> Result<Vec<ReviewedEntity>, String> {
    let state = vault.0.lock().map_err(|_| "The vault is unavailable")?;
    let identity = state
        .as_ref()
        .ok_or("Unlock the Vault first")?
        .data
        .pseudonyms
        .iter()
        .find(|identity| identity.id == id)
        .ok_or("Saved identity not found")?;
    find_matches(identity, &text)
}

#[tauri::command]
pub fn restore_vault_record(id: String, vault: State<'_, VaultState>) -> Result<String, String> {
    vault
        .0
        .lock()
        .map_err(|_| "The vault is unavailable")?
        .as_ref()
        .ok_or("Unlock the Vault to restore records")?
        .data
        .restore(&id)
}

#[tauri::command]
pub fn clear_vault_records(vault: State<'_, VaultState>) -> Result<(), String> {
    vault
        .0
        .lock()
        .map_err(|_| "The vault is unavailable")?
        .as_mut()
        .ok_or("Unlock the Vault first")?
        .update(|data| {
            data.records.clear();
            Ok(())
        })
}

fn rendered_pdf(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    use std::io::Write;
    use std::process::Stdio;
    let mut child = crate::engine_command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Unable to start the local PDF exporter")?;
    let request = serde_json::json!({"action": "export_pdf", "text": text});
    let sent = child
        .stdin
        .take()
        .ok_or("Unable to communicate with PDF exporter")
        .and_then(|mut stdin| {
            stdin
                .write_all(request.to_string().as_bytes())
                .map_err(|_| "Unable to send PDF export request")
        });
    if let Err(error) = sent {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let output = child.wait_with_output().map_err(|_| "PDF export failed")?;
    if !output.status.success() {
        return Err("Unable to render the processed text as PDF".into());
    }
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "Invalid PDF exporter response")?;
    base64::engine::general_purpose::STANDARD
        .decode(response["pdf"].as_str().ok_or("Missing PDF output")?)
        .map_err(|_| "Invalid PDF output".into())
}

#[tauri::command]
pub async fn export_processed_text(
    app: tauri::AppHandle,
    text: String,
    format: Option<String>,
) -> Result<bool, String> {
    use std::io::Write;
    let format = format.unwrap_or_else(|| "txt".into());
    if text.len() > 5_000_000 || !matches!(format.as_str(), "txt" | "pdf") {
        return Err("Choose TXT or PDF within the export size limit".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app
            .dialog()
            .file()
            .add_filter(&format.to_uppercase(), &[format.as_str()])
            .set_file_name(format!("veil-processed.{format}"))
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = file.into_path().map_err(|_| "Choose a local file")?;
        let path = path.with_extension(&format);
        let bytes = if format == "pdf" {
            rendered_pdf(&text)?
        } else {
            text.into_bytes()
        };
        let mut pending =
            tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid output folder")?)
                .map_err(|_| "Unable to create the output file")?;
        pending
            .write_all(&bytes)
            .and_then(|_| pending.as_file().sync_all())
            .map_err(|_| "Unable to save the output file")?;
        pending
            .persist(path)
            .map_err(|_| "Unable to replace the output file")?;
        Ok(true)
    })
    .await
    .map_err(|_| "The export task stopped unexpectedly".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(id: &str, value: &str, start: usize) -> ReviewedEntity {
        ReviewedEntity {
            id: id.into(),
            entity_type: "PERSON".into(),
            text: value.into(),
            start,
            end: start + value.chars().count(),
        }
    }

    #[test]
    fn anonymization_replaces_only_reviewed_spans_and_cannot_restore() {
        let mut vault = SessionVault::default();
        let result = vault
            .process(
                "John met Jane".into(),
                vec![entity("1", "John", 0)],
                "anonymize",
            )
            .unwrap();
        assert_eq!(result.text, "[REDACTED] met Jane");
        assert!(vault.restore(&result.id).is_err());
        assert!(vault.pseudonyms.is_empty());
        assert!(vault.records[0].original.is_none());
    }

    #[test]
    fn repeats_share_a_pseudonym_across_documents_and_restore_exactly() {
        let mut vault = SessionVault::default();
        let original = "John met John";
        let result = vault
            .process(
                original.into(),
                vec![entity("1", "John", 0), entity("2", "John", 9)],
                "pseudonymize",
            )
            .unwrap();
        let parts: Vec<_> = result.text.split(" met ").collect();
        assert_eq!(parts[0], parts[1]);
        assert!(!result.text.contains("John"));
        let other = vault
            .process("John".into(), vec![entity("1", "John", 0)], "pseudonymize")
            .unwrap();
        assert_eq!(other.text, parts[0]);
        assert_eq!(vault.restore(&result.id).unwrap(), original);
    }

    #[test]
    fn unicode_and_adjacent_spans_preserve_positions() {
        let mut vault = SessionVault::default();
        let result = vault
            .process(
                "😀عليZoë!".into(),
                vec![entity("2", "Zoë", 4), entity("1", "علي", 1)],
                "anonymize",
            )
            .unwrap();
        assert_eq!(result.text, "😀[REDACTED][REDACTED]!");
    }

    #[test]
    fn invalid_overlap_stale_empty_and_mode_requests_do_not_change_vault() {
        let mut vault = SessionVault::default();
        assert!(
            vault
                .process(
                    "John".into(),
                    vec![entity("1", "John", 0), entity("2", "ohn", 1)],
                    "pseudonymize"
                )
                .is_err()
        );
        assert!(
            vault
                .process("Jane".into(), vec![entity("1", "John", 0)], "pseudonymize")
                .is_err()
        );
        assert!(
            vault
                .process("John".into(), vec![], "pseudonymize")
                .is_err()
        );
        assert!(
            vault
                .process("John".into(), vec![entity("1", "John", 0)], "invalid")
                .is_err()
        );
        assert!(vault.records.is_empty());
        assert!(vault.pseudonyms.is_empty());
    }

    #[test]
    fn categories_and_distinct_people_receive_distinct_tokens() {
        let mut vault = SessionVault::default();
        let john = vault
            .process("John".into(), vec![entity("1", "John", 0)], "pseudonymize")
            .unwrap();
        let jane = vault
            .process("Jane".into(), vec![entity("1", "Jane", 0)], "pseudonymize")
            .unwrap();
        let mut organization = entity("1", "John", 0);
        organization.entity_type = "ORGANIZATION".into();
        let company = vault
            .process("John".into(), vec![organization], "pseudonymize")
            .unwrap();
        assert_ne!(john.text, jane.text);
        assert_ne!(john.text, company.text);
    }

    #[test]
    fn missing_record_and_clear_session_cannot_restore() {
        let mut vault = SessionVault::default();
        let result = vault
            .process("John".into(), vec![entity("1", "John", 0)], "pseudonymize")
            .unwrap();
        assert!(vault.restore("unknown").is_err());
        vault = SessionVault::default();
        assert!(vault.restore(&result.id).is_err());
        assert!(vault.pseudonyms.is_empty());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    #[ignore = "requires the local Python engine and English model"]
    fn detect_review_process_and_restore_flow() {
        let original = "😀 Send to mira@example.com or copy mira@example.com.";
        let detected = crate::detect_pii(original.into(), vec!["EMAIL_ADDRESS".into()]).unwrap();
        let hits = detected["detections"].as_array().unwrap();
        assert_eq!(hits.len(), 2);
        let reviewed: Vec<ReviewedEntity> = hits
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                let mut value = hit.clone();
                value["id"] = format!("detection-{index}").into();
                serde_json::from_value(value).unwrap()
            })
            .collect();

        let mut vault = SessionVault::default();
        // Simulate rejecting the first detection and accepting the second.
        let anonymous = vault
            .process(original.into(), vec![reviewed[1].clone()], "anonymize")
            .unwrap();
        assert_eq!(
            anonymous.text,
            "😀 Send to mira@example.com or copy [REDACTED]."
        );
        assert!(vault.restore(&anonymous.id).is_err());
        let pseudo = vault
            .process(original.into(), reviewed, "pseudonymize")
            .unwrap();
        assert!(!pseudo.text.contains("mira@example.com"));
        assert_eq!(vault.restore(&pseudo.id).unwrap(), original);
        assert_eq!(vault.pseudonyms.len(), 1);
    }
}

#[tauri::command]
pub async fn set_vault_protection(
    window: tauri::Window,
    use_account: bool,
    require_authentication: Option<bool>,
    password: String,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    let password = Zeroizing::new(password);
    let require_authentication = require_authentication.unwrap_or(false);
    let previously_required = vault
        .0
        .lock()
        .map_err(|_| "The Vault is unavailable")?
        .as_ref()
        .ok_or("Unlock the Vault before changing its protection")?
        .authentication_required;
    // Verify before enabling system sign-in, and before removing an existing
    // system sign-in requirement. Cancellation never changes saved protection.
    if require_authentication || previously_required {
        crate::os_auth::verify(window).await?;
    }
    vault
        .0
        .lock()
        .map_err(|_| "The Vault is unavailable")?
        .as_mut()
        .ok_or("Unlock the Vault before changing its protection")?
        .change_protection_with_auth(use_account, &password, require_authentication)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DetectionRule {
    pub phrase: String,
    pub entity_type: String,
}

#[tauri::command]
pub fn save_detection_rules(
    rules: Vec<DetectionRule>,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    if rules.len() > 200
        || rules.iter().any(|rule| {
            rule.phrase.trim().is_empty()
                || rule.phrase.len() > 500
                || !crate::review::supported_entity(&rule.entity_type)
        })
    {
        return Err(
            "Use up to 200 non-empty phrases of at most 500 bytes with valid categories".into(),
        );
    }
    vault
        .0
        .lock()
        .map_err(|_| "The Vault is unavailable")?
        .as_mut()
        .ok_or("Unlock the Vault before saving detection rules")?
        .update(|data| {
            data.rules = rules;
            Ok(())
        })
}

#[tauri::command]
pub fn list_detection_rules(vault: State<'_, VaultState>) -> Result<Vec<DetectionRule>, String> {
    Ok(vault
        .0
        .lock()
        .map_err(|_| "The Vault is unavailable")?
        .as_ref()
        .ok_or("Unlock the Vault first")?
        .data
        .rules
        .clone())
}

#[tauri::command]
pub async fn detect_with_rules(
    text: String,
    entities: Vec<String>,
    vault: State<'_, VaultState>,
) -> Result<serde_json::Value, String> {
    let rules = vault
        .0
        .lock()
        .map_err(|_| "The Vault is unavailable")?
        .as_ref()
        .map(|opened| opened.data.rules.clone())
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || crate::run_detection(text, entities, rules))
        .await
        .map_err(|_| "The local detection task stopped unexpectedly".to_string())?
}

#[cfg(test)]
mod new_workflow_tests {
    #[test]
    fn resetting_requires_exact_explicit_confirmation() {
        assert!(super::check_reset_confirmation("").is_err());
        assert!(super::check_reset_confirmation("reset vault").is_err());
        assert!(super::check_reset_confirmation("RESET VAULT").is_ok());
    }

    #[test]
    fn all_new_categories_validate_and_receive_short_pseudonyms() {
        let categories = [
            "STREET_ADDRESS",
            "POSTAL_CODE",
            "DATE_TIME",
            "CREDIT_CARD",
            "IBAN_CODE",
            "IP_ADDRESS",
            "URL",
            "ID_NUMBER",
            "INDIRECT_IDENTIFIER",
        ];
        let mut data = super::SessionVault::default();
        for category in categories {
            let result = data
                .process(
                    "Example".into(),
                    vec![crate::review::ReviewedEntity {
                        id: "1".into(),
                        entity_type: category.into(),
                        text: "Example".into(),
                        start: 0,
                        end: 7,
                    }],
                    "pseudonymize",
                )
                .unwrap();
            assert!(result.text.ends_with("_001"));
            assert!(result.text.len() <= 12);
        }
    }

    #[test]
    #[ignore = "requires the local Python PDF export dependency"]
    fn native_export_bridge_returns_a_real_pdf() {
        let pdf = super::rendered_pdf("PERSON_001\nEMAIL_001").unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert!(pdf.len() > 500);
    }
}

#[cfg(test)]
mod large_text_processing_tests {
    #[test]
    fn larger_text_processes_and_restores_without_truncation() {
        let source = format!("{}John Smith", "ordinary text ".repeat(10_000));
        assert!(source.len() > 100_000);
        let end = source.chars().count();
        let mut vault = super::SessionVault::default();
        let result = vault
            .process(
                source.clone(),
                vec![crate::review::ReviewedEntity {
                    id: "large-tail".into(),
                    entity_type: "PERSON".into(),
                    text: "John Smith".into(),
                    start: end - 10,
                    end,
                }],
                "pseudonymize",
            )
            .unwrap();
        assert!(result.text.ends_with("PERSON_001"));
        assert_eq!(vault.restore(&result.id).unwrap(), source);
        assert!(
            vault
                .process("x".repeat(500_001), vec![], "anonymize")
                .is_err()
        );
        assert_eq!(vault.records.len(), 1);
    }
}
