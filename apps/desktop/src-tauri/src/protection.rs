use crate::review::ReviewedEntity;
use crate::database::{self, Mode};
#[cfg(test)]
use crate::vault_storage::UnlockedVault;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::OnceLock;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use zeroize::Zeroizing;

#[derive(Clone, Serialize, Deserialize)]
pub struct ProcessedRecord {
    pub(crate) id: String,
    pub(crate) mode: String,
    pub(crate) text: String,
    pub(crate) count: usize,
    #[serde(skip)]
    pub(crate) export_replacements: Vec<crate::docx::Replacement>,
}

#[derive(Serialize)]
pub struct ProcessedResponse {
    #[serde(flatten)]
    record: ProcessedRecord,
    replacements: Vec<crate::docx::Replacement>,
}

impl From<ProcessedRecord> for ProcessedResponse {
    fn from(mut record: ProcessedRecord) -> Self {
        let replacements = std::mem::take(&mut record.export_replacements);
        Self {
            record,
            replacements,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct StoredRecord {
    pub(crate) public: ProcessedRecord,
    pub(crate) original: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedIdentity {
    pub(crate) id: String,
    pub(crate) entity_type: String,
    pub(crate) original: String,
    pub(crate) pseudonym: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SessionVault {
    #[cfg(test)]
    #[serde(skip)]
    project_index: OnceLock<HashMap<String, usize>>,
    #[cfg(test)]
    #[serde(skip)]
    identity_index: OnceLock<HashMap<(String, String), usize>>,
    #[serde(default)]
    pub(crate) projects: Vec<VaultProject>,
    pub(crate) records: Vec<StoredRecord>,
    pub(crate) pseudonyms: Vec<SavedIdentity>,
    #[serde(default)]
    pub(crate) counters: HashMap<String, usize>,
    #[serde(default)]
    pub(crate) rules: Vec<DetectionRule>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct VaultProject {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) data: SessionVault,
}

#[derive(Serialize)]
pub struct ProjectSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) documents: usize,
    pub(crate) identities: usize,
}

#[cfg(test)]
impl SessionVault {
    fn project_position(&self, id: &str) -> Result<usize, String> {
        self.project_index
            .get_or_init(|| {
                self.projects
                    .iter()
                    .enumerate()
                    .map(|(index, project)| (project.id.clone(), index))
                    .collect()
            })
            .get(id)
            .copied()
            .ok_or_else(|| "Project not found. Select an existing project.".into())
    }

    fn project(&self, id: Option<&str>) -> Result<&SessionVault, String> {
        match id.unwrap_or("default") {
            "default" => Ok(self),
            id => Ok(&self.projects[self.project_position(id)?].data),
        }
    }

    pub(crate) fn update_project<T>(
        &mut self,
        id: Option<&str>,
        change: impl FnOnce(&mut SessionVault) -> Result<T, String>,
    ) -> Result<T, String> {
        if id.unwrap_or("default") == "default" {
            return change(self);
        }
        let position = self.project_position(id.unwrap_or("default"))?;
        let project = &mut self.projects[position];
        // Allocate tokens across the Vault, but resolve identities only within the project.
        project.data.counters = self.counters.clone();
        let result = change(&mut project.data)?;
        self.counters = project.data.counters.clone();
        Ok(result)
    }

    pub(crate) fn create_project(&mut self, name: String) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err("Enter a project name of 1–80 characters".into());
        }
        if self.projects.len() >= 100 {
            return Err("The Vault supports up to 100 projects".into());
        }
        if name.eq_ignore_ascii_case("General")
            || self
                .projects
                .iter()
                .any(|p| p.name.to_lowercase() == name.to_lowercase())
        {
            return Err("A project with this name already exists".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.project_index.take();
        self.projects.push(VaultProject {
            id: id.clone(),
            name: name.into(),
            data: SessionVault::default(),
        });
        Ok(id)
    }
    pub(crate) fn delete_project(&mut self, id: &str, confirmation: &str) -> Result<(), String> {
        if id == "default" {
            return Err("The General project cannot be deleted".into());
        }
        let position = self.project_position(id)?;
        if confirmation != self.projects[position].name {
            return Err("Type the project name exactly to confirm deletion".into());
        }
        self.projects.remove(position);
        self.project_index.take();
        // Vault-wide counters remain reserved after deleting a project.
        Ok(())
    }
}

#[derive(Default)]
pub struct VaultState(Mutex<VaultSession>);

#[derive(Default)]
struct VaultSession {
    opened: Option<database::Vault>,
    checked_auto_open: bool,
    epoch: u64,
}

impl VaultSession {
    fn advance(&mut self) -> Result<(), String> {
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or("Vault session generation exhausted")?;
        Ok(())
    }

    fn verify_epoch(&self, expected: u64) -> Result<(), String> {
        if self.epoch != expected {
            return Err("The Vault session changed. Start the action again.".into());
        }
        Ok(())
    }
}

impl VaultState {
    fn enter(&self) -> Result<std::sync::MutexGuard<'_, VaultSession>, String> {
        self.0.lock().map_err(|_| "The Vault is unavailable".into())
    }

    fn read<T>(
        &self,
        read: impl FnOnce(&database::Vault) -> Result<T, String>,
    ) -> Result<T, String> {
        read(
            self.enter()?
                .opened
                .as_ref()
                .ok_or("Unlock the Vault first")?,
        )
    }

    fn write<T>(
        &self,
        change: impl FnOnce(&mut database::Vault) -> Result<T, String>,
    ) -> Result<T, String> {
        change(
            self.enter()?
                .opened
                .as_mut()
                .ok_or("Unlock the Vault first")?,
        )
    }
}

#[tauri::command]
pub fn list_projects(vault: State<'_, VaultState>) -> Result<Vec<ProjectSummary>, String> {
    vault.read(|opened| opened.repo.projects())
}

#[tauri::command]
pub fn create_project(name: String, vault: State<'_, VaultState>) -> Result<String, String> {
    vault.write(|opened| opened.repo.create_project(name))
}

#[tauri::command]
pub fn delete_project(
    project_id: String,
    confirmation: String,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    vault.write(|opened| opened.repo.delete_project(&project_id, &confirmation))
}

pub(crate) fn identity_key(category: &str, value: &str) -> String {
    match category {
        "PERSON" | "LOCATION" | "ORGANIZATION" => value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase(),
        _ => value.trim().to_string(),
    }
}

pub(crate) const CATEGORY_PREFIXES: &[(&str, &str)] = &[
    ("PERSON", "PERSON"),
    ("EMAIL_ADDRESS", "EMAIL"),
    ("PHONE_NUMBER", "PHONE"),
    ("LOCATION", "LOCATION"),
    ("ORGANIZATION", "ORG"),
    ("STREET_ADDRESS", "ADDRESS"),
    ("POSTAL_CODE", "POSTCODE"),
    ("DATE_TIME", "DATE"),
    ("CREDIT_CARD", "CARD"),
    ("IBAN_CODE", "IBAN"),
    ("IP_ADDRESS", "IP"),
    ("URL", "URL"),
    ("ID_NUMBER", "ID"),
    ("INDIRECT_IDENTIFIER", "DETAIL"),
];

pub(crate) fn category_prefix(category: &str) -> Option<&'static str> {
    CATEGORY_PREFIXES
        .iter()
        .find(|(code, _)| *code == category)
        .map(|(_, prefix)| *prefix)
}

pub(crate) fn project_name_key(name: &str) -> Result<String, String> {
    if name.is_empty() || name.chars().count() > 80 {
        return Err("Enter a project name of 1–80 characters".into());
    }
    if name.eq_ignore_ascii_case("General") {
        return Err("A project with this name already exists".into());
    }
    Ok(name.to_lowercase())
}

pub(crate) fn mapping_value<'a>(
    category: &str,
    value: &'a str,
) -> Result<(&'a str, String), String> {
    if category_prefix(category).is_none() {
        return Err("Unsupported identity category".into());
    }
    identity_value(category, value)
}

fn identity_value<'a>(category: &str, value: &'a str) -> Result<(&'a str, String), String> {
    let original = value.trim();
    if original.is_empty() || original.len() > 10_000 {
        return Err("Enter an identity between 1 and 10,000 bytes".into());
    }
    Ok((original, identity_key(category, original)))
}

pub(crate) fn next_token(
    mut counter: i64,
    prefix: &str,
    source: &str,
) -> Result<(i64, String), String> {
    loop {
        counter = counter
            .checked_add(1)
            .ok_or("Pseudonym counter is exhausted")?;
        let candidate = format!("{prefix}_{counter:03}");
        if !source.contains(&candidate) {
            return Ok((counter, candidate));
        }
    }
}

#[derive(Default)]
struct TemporaryMappings {
    tokens: HashMap<(String, String), String>,
    counters: HashMap<String, i64>,
}

impl TemporaryMappings {
    fn token(&mut self, entity: &ReviewedEntity, source: &str) -> Result<String, String> {
        let prefix = if crate::review::custom_entity(&entity.entity_type) {
            entity.entity_type.as_str()
        } else {
            category_prefix(&entity.entity_type).ok_or("Unsupported identity category")?
        };
        let (_, value) = identity_value(&entity.entity_type, &entity.text)?;
        let key = (entity.entity_type.clone(), value);
        if let Some(token) = self.tokens.get(&key) {
            return Ok(token.clone());
        }
        let counter = self.counters.entry(entity.entity_type.clone()).or_default();
        let (next, token) = next_token(*counter, prefix, source)?;
        *counter = next;
        self.tokens.insert(key, token.clone());
        Ok(token)
    }
}

/// The document workflow selects the mapping component and its storage boundary.
#[derive(Clone, Copy)]
pub(crate) enum MappingMode {
    Document,
    Vault,
}

impl MappingMode {
    fn resolve(mode: &str) -> Result<Self, String> {
        match mode {
            "anonymize" => Ok(Self::Document),
            "pseudonymize" => Ok(Self::Vault),
            _ => Err("Choose a valid processing mode".into()),
        }
    }

    pub(crate) fn components() -> [serde_json::Value; 2] {
        [
            serde_json::json!({"id": "document-mapping", "name": "Document-local tokens",
                "capabilities": ["entity_mapping"], "mandatory": true}),
            serde_json::json!({"id": "vault-mapping", "name": "Encrypted Vault identities",
                "capabilities": ["entity_mapping"], "mandatory": true}),
        ]
    }
}

trait EntityMapper {
    fn map_entity(&mut self, entity: &ReviewedEntity) -> Result<String, String>;
}

impl<F: FnMut(&ReviewedEntity) -> Result<String, String>> EntityMapper for F {
    fn map_entity(&mut self, entity: &ReviewedEntity) -> Result<String, String> {
        self(entity)
    }
}

pub(crate) fn approved_detections(
    text: &str,
    detections: Vec<ReviewedEntity>,
    mode: &str,
) -> Result<Vec<ReviewedEntity>, String> {
    let mapping = MappingMode::resolve(mode)?;
    let approved = crate::review::validate_spans(text, detections)?;
    if matches!(mapping, MappingMode::Vault)
        && approved.iter().any(|span| crate::review::custom_entity(&span.entity_type))
    {
        return Err("Custom categories are supported in anonymization only".into());
    }
    if approved.is_empty() {
        return Err("Accept at least one detection before processing".into());
    }
    Ok(approved)
}

pub(crate) fn render_processed(
    text: &str,
    approved: &[ReviewedEntity],
    mode: &str,
    mut replacement: impl FnMut(&ReviewedEntity) -> Result<String, String>,
) -> Result<ProcessedRecord, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::new();
    let mut cursor = 0;
    let mut export_replacements = Vec::with_capacity(approved.len());
    for entity in approved {
        output.extend(chars[cursor..entity.start].iter());
        let token = replacement.map_entity(entity)?;
        output.push_str(&token);
        export_replacements.push(crate::docx::Replacement {
            start: entity.start,
            end: entity.end,
            text: entity.text.clone(),
            replacement: token,
        });
        cursor = entity.end;
    }
    output.extend(chars[cursor..].iter());
    Ok(ProcessedRecord {
        id: uuid::Uuid::new_v4().to_string(),
        mode: mode.into(),
        text: output,
        count: approved.len(),
        export_replacements,
    })
}

#[cfg(test)]
impl SessionVault {
    pub(crate) fn save_identity(
        &mut self,
        category: &str,
        original: &str,
        source: &str,
    ) -> Result<SavedIdentity, String> {
        let prefix = category_prefix(category).ok_or("Unsupported identity category")?;
        let (original, normalized) = mapping_value(category, original)?;
        let key = (category.to_string(), normalized);
        let index = self.identity_index.get_or_init(|| {
            let mut index = HashMap::new();
            for (position, identity) in self.pseudonyms.iter().enumerate() {
                index
                    .entry((
                        identity.entity_type.clone(),
                        identity_key(&identity.entity_type, &identity.original),
                    ))
                    .or_insert(position);
            }
            index
        });
        if let Some(position) = index.get(&key) {
            return Ok(self.pseudonyms[*position].clone());
        }
        let counter = self.counters.entry(category.into()).or_default();
        let (next, pseudonym) = next_token(
            i64::try_from(*counter).map_err(|_| "Pseudonym counter is exhausted")?,
            prefix,
            source,
        )?;
        *counter = usize::try_from(next).map_err(|_| "Pseudonym counter is exhausted")?;
        let identity = SavedIdentity {
            id: uuid::Uuid::new_v4().to_string(),
            entity_type: category.into(),
            original: original.into(),
            pseudonym,
        };
        self.identity_index
            .get_mut()
            .expect("identity index initialized")
            .insert(key, self.pseudonyms.len());
        self.pseudonyms.push(identity.clone());
        Ok(identity)
    }

    pub(crate) fn process(
        &mut self,
        text: String,
        detections: Vec<ReviewedEntity>,
        mode: &str,
    ) -> Result<ProcessedRecord, String> {
        let approved = approved_detections(&text, detections, mode)?;
        if mode == "pseudonymize" && self.records.len() >= 100 {
            return Err("Document history is full. Clear history in the Vault; saved identities will be kept.".into());
        }
        let mut temporary = TemporaryMappings::default();
        let record = render_processed(&text, &approved, mode, |entity| {
            if mode == "anonymize" {
                temporary.token(entity, &text)
            } else {
                self.save_identity(&entity.entity_type, &entity.text, &text)
                    .map(|identity| identity.pseudonym)
            }
        })?;
        if mode == "anonymize" {
            return Ok(record);
        }
        self.records.push(StoredRecord {
            public: ProcessedRecord {
                export_replacements: Vec::new(),
                ..record.clone()
            },
            original: (mode == "pseudonymize").then_some(text),
        });
        Ok(record)
    }

    fn delete_identity(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .pseudonyms
            .iter()
            .position(|identity| identity.id == id)
            .ok_or("Saved mapping not found in this project")?;
        self.pseudonyms.remove(index);
        self.identity_index.take();
        // Keep counters monotonic: old document tokens must never be reassigned.
        Ok(())
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
pub async fn vault_status(app: tauri::AppHandle) -> Result<VaultStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = vault_path(&app)?;
        current_vault_status(path, &app.state::<VaultState>())
    })
    .await
    .map_err(|_| "Unable to read Vault status")?
}

fn current_vault_status(
    path: std::path::PathBuf,
    vault: &VaultState,
) -> Result<VaultStatus, String> {
    let mut session = vault.enter()?;
    let mut needs_migration = false;
    let (exists, mode) = if let Some(opened) = &session.opened {
        (true, Some(opened.mode()))
    } else {
        match database::inspect(&path) {
            Ok(snapshot) => {
                let mode = snapshot.mode();
                needs_migration = snapshot.needs_migration();
                if !session.checked_auto_open
                    && mode == Some(Mode::Account)
                    && Mode::account_supported()
                {
                    session.checked_auto_open = true;
                    session.advance()?;
                    session.opened = Some(database::Vault::open(
                        path,
                        snapshot,
                        "",
                        false,
                        Mode::Account,
                    )?);
                    needs_migration = false;
                }
                (mode.is_some(), mode)
            }
            // Report an existing Vault so the UI never offers to create over corruption.
            Err(_) => (true, None),
        }
    };
    session.checked_auto_open = true;
    Ok(VaultStatus {
        exists,
        authentication_required: mode == Some(Mode::System),
        needs_migration,
        code_required: exists && (mode.is_none() || mode == Some(Mode::Passphrase)),
        account_supported: Mode::account_supported(),
        unlocked: session.opened.is_some(),
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
    let epoch = {
        let session = vault.enter()?;
        if session.opened.is_some() {
            return Err("The Vault is already unlocked".into());
        }
        session.epoch
    };
    let snapshot = database::inspect(&path)?;
    let mode = match snapshot.mode() {
        Some(mode) => mode,
        None => Mode::from_options(
            use_account.unwrap_or(Mode::account_supported()),
            require_authentication.unwrap_or(false),
        )?,
    };
    if mode.requires_authentication() {
        crate::os_auth::verify(window).await?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<VaultState>();
        let mut session = state.enter()?;
        session.verify_epoch(epoch)?;
        if session.opened.is_some() {
            return Err("The Vault is already unlocked".into());
        }
        session.advance()?;
        session.checked_auto_open = true;
        session.opened = Some(database::Vault::open(
            path, snapshot, &password, create, mode,
        )?);
        Ok(())
    })
    .await
    .map_err(|_| "Vault unlock could not finish")?
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
    let mut session = vault.enter()?;
    session.advance()?;
    if let Some(opened) = session.opened.as_mut() {
        opened.repo.reset()?;
    } else {
        session.opened = Some(database::Vault::reset_locked(vault_path(&app)?)?);
    }
    session.checked_auto_open = true;
    Ok(())
}

#[tauri::command]
pub fn lock_vault(vault: State<'_, VaultState>) -> Result<(), String> {
    let mut session = vault.enter()?;
    session.advance()?;
    session.checked_auto_open = true;
    session.opened = None;
    Ok(())
}

#[tauri::command]
pub fn process_reviewed_text(
    project_id: Option<String>,
    text: String,
    detections: Vec<ReviewedEntity>,
    mode: String,
    operation_id: Option<String>,
    vault: State<'_, VaultState>,
) -> Result<ProcessedResponse, String> {
    let approved = approved_detections(&text, detections, &mode)?;
    let record = match MappingMode::resolve(&mode)? {
        MappingMode::Document => {
            let mut identities = TemporaryMappings::default();
            render_processed(&text, &approved, &mode, |entity| identities.token(entity, &text))?
        }
        MappingMode::Vault => {
            let operation_id = operation_id
                .as_deref()
                .map(uuid::Uuid::parse_str)
                .transpose()
                .map_err(|_| "Invalid processing request identifier")?;
            vault.write(|opened| opened.repo.process(scope(&project_id), text, approved, operation_id))?
        }
    };
    Ok(record.into())
}

fn scope(project: &Option<String>) -> &str {
    project.as_deref().unwrap_or("default")
}

#[tauri::command]
pub fn list_vault_records(
    project_id: Option<String>,
    vault: State<'_, VaultState>,
) -> Result<Vec<ProcessedRecord>, String> {
    vault.read(|opened| opened.repo.records(scope(&project_id)))
}

#[tauri::command]
pub fn list_saved_identities(
    project_id: Option<String>,
    vault: State<'_, VaultState>,
) -> Result<Vec<SavedIdentity>, String> {
    vault.read(|opened| opened.repo.mappings(scope(&project_id)))
}

#[tauri::command]
pub fn save_identity(
    project_id: Option<String>,
    original: String,
    entity_type: String,
    vault: State<'_, VaultState>,
) -> Result<SavedIdentity, String> {
    vault.write(|opened| {
        opened
            .repo
            .save_mapping(scope(&project_id), &entity_type, &original)
    })
}

#[tauri::command]
pub fn delete_saved_identity(
    project_id: Option<String>,
    id: String,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    vault.write(|opened| opened.repo.delete_mapping(scope(&project_id), &id))
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
    project_id: Option<String>,
    id: String,
    text: String,
    vault: State<'_, VaultState>,
) -> Result<Vec<ReviewedEntity>, String> {
    vault.read(|opened| find_matches(&opened.repo.mapping(scope(&project_id), &id)?, &text))
}

#[tauri::command]
pub fn restore_vault_record(
    project_id: Option<String>,
    id: String,
    vault: State<'_, VaultState>,
) -> Result<String, String> {
    vault.read(|opened| opened.repo.restore(scope(&project_id), &id))
}

#[tauri::command]
pub fn clear_vault_records(
    project_id: Option<String>,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    vault.write(|opened| opened.repo.clear_history(scope(&project_id)))
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
    fn processing_returns_exact_transient_replacements_in_both_modes() {
        for mode in ["anonymize", "pseudonymize"] {
            let mut vault = SessionVault::default();
            let record = vault.process(
                "😀 John met John".into(), vec![entity("second", "John", 11)], mode,
            ).unwrap();
            let response = ProcessedResponse::from(record);
            assert_eq!(response.replacements.len(), 1);
            assert_eq!(response.replacements[0].start, 11);
            assert_eq!(response.replacements[0].text, "John");
            assert_eq!(response.replacements[0].replacement, "PERSON_001");
            assert_eq!(response.record.text, "😀 John met PERSON_001");
            if mode == "pseudonymize" {
                assert!(vault.records[0].public.export_replacements.is_empty());
            }
        }
    }

    #[test]
    fn anonymization_replaces_only_reviewed_spans_without_saving_history() {
        let mut vault = SessionVault::default();
        let result = vault
            .process(
                "John met Jane".into(),
                vec![entity("1", "John", 0)],
                "anonymize",
            )
            .unwrap();
        assert_eq!(result.text, "PERSON_001 met Jane");
        assert!(vault.restore(&result.id).is_err());
        assert!(vault.pseudonyms.is_empty());
        assert!(vault.records.is_empty());
    }

    #[test]
    fn custom_categories_use_document_tokens_and_are_rejected_by_the_vault() {
        let source = "α Ω-123 met Ω-123";
        let spans = vec![
            ReviewedEntity {
                entity_type: "CUSTOM_ACCOUNT_ID".into(),
                ..entity("custom-1", "Ω-123", 2)
            },
            ReviewedEntity {
                entity_type: "CUSTOM_ACCOUNT_ID".into(),
                ..entity("custom-2", "Ω-123", 12)
            },
        ];
        let mut vault = SessionVault::default();
        let record = vault.process(source.into(), spans.clone(), "anonymize").unwrap();
        assert_eq!(record.text, "α CUSTOM_ACCOUNT_ID_001 met CUSTOM_ACCOUNT_ID_001");
        assert!(matches!(vault.process(source.into(), spans, "pseudonymize"),
            Err(message) if message.contains("anonymization only")));
        assert!(vault.records.is_empty());
        assert!(vault.pseudonyms.is_empty());
    }

    #[test]
    fn anonymization_reuses_document_tokens_for_all_categories() {
        let mut vault = SessionVault::default();
        for (category, label) in [
            ("PERSON", "PERSON"),
            ("EMAIL_ADDRESS", "EMAIL"),
            ("PHONE_NUMBER", "PHONE"),
            ("LOCATION", "LOCATION"),
            ("ORGANIZATION", "ORG"),
            ("STREET_ADDRESS", "ADDRESS"),
            ("POSTAL_CODE", "POSTCODE"),
            ("DATE_TIME", "DATE"),
            ("CREDIT_CARD", "CARD"),
            ("IBAN_CODE", "IBAN"),
            ("IP_ADDRESS", "IP"),
            ("URL", "URL"),
            ("ID_NUMBER", "ID"),
            ("INDIRECT_IDENTIFIER", "DETAIL"),
        ] {
            let mut first = entity("1", "John", 0);
            first.entity_type = category.into();
            let mut second = entity("2", "John", 9);
            second.entity_type = category.into();
            let result = vault
                .process("John met John".into(), vec![first, second], "anonymize")
                .unwrap();
            assert_eq!(result.text, format!("{label}_001 met {label}_001"));
        }
        assert!(vault.records.is_empty());
        assert!(vault.pseudonyms.is_empty());
    }

    #[test]
    fn anonymization_distinguishes_people_and_resets_between_documents() {
        let mut vault = SessionVault::default();
        let result = vault
            .process(
                "John met John and Hiba".into(),
                vec![
                    entity("1", "John", 0),
                    entity("2", "John", 9),
                    entity("3", "Hiba", 18),
                ],
                "anonymize",
            )
            .unwrap();
        assert_eq!(result.text, "PERSON_001 met PERSON_001 and PERSON_002");
        let next = vault
            .process("Hiba".into(), vec![entity("1", "Hiba", 0)], "anonymize")
            .unwrap();
        assert_eq!(next.text, "PERSON_001");
        assert!(vault.pseudonyms.is_empty());
    }

    #[test]
    fn anonymization_leaves_existing_full_vault_unchanged() {
        let mut vault = SessionVault::default();
        let saved = vault
            .process("John".into(), vec![entity("1", "John", 0)], "pseudonymize")
            .unwrap();
        vault.records = vec![vault.records[0].clone(); 100];
        let before = serde_json::to_value(&vault).unwrap();
        let output = vault
            .process("John".into(), vec![entity("1", "John", 0)], "anonymize")
            .unwrap();
        assert_eq!(output.text, "PERSON_001");
        assert_eq!(serde_json::to_value(&vault).unwrap(), before);
        assert!(vault.restore(&output.id).is_err());
        assert_eq!(vault.restore(&saved.id).unwrap(), "John");
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
        assert_eq!(result.text, "😀PERSON_001PERSON_002!");
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
        let detected = crate::detect_default(original.into(), vec!["EMAIL_ADDRESS".into()]).unwrap();
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
            "😀 Send to mira@example.com or copy EMAIL_001."
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
    app: tauri::AppHandle,
    window: tauri::Window,
    use_account: bool,
    require_authentication: Option<bool>,
    password: String,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    let password = Zeroizing::new(password);
    let mode = Mode::from_options(use_account, require_authentication.unwrap_or(false))?;
    let (epoch, old_mode) = {
        let session = vault.enter()?;
        (
            session.epoch,
            session
                .opened
                .as_ref()
                .ok_or("Unlock the Vault before changing its protection")?
                .mode(),
        )
    };
    if mode.requires_authentication() || old_mode.requires_authentication() {
        crate::os_auth::verify(window).await?;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<VaultState>();
        let mut session = state.enter()?;
        session.verify_epoch(epoch)?;
        session.advance()?;
        session
            .opened
            .as_mut()
            .ok_or("Unlock the Vault before changing its protection")?
            .change_protection(mode, &password)
    })
    .await
    .map_err(|_| "Unable to finish changing Vault protection")?
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DetectionRule {
    pub phrase: String,
    pub entity_type: String,
}

pub(crate) fn validate_rules(rules: &[DetectionRule]) -> Result<(), String> {
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
    Ok(())
}

#[tauri::command]
pub fn save_detection_rules(
    project_id: Option<String>,
    rules: Vec<DetectionRule>,
    vault: State<'_, VaultState>,
) -> Result<(), String> {
    vault.write(|opened| opened.repo.replace_rules(scope(&project_id), &rules))
}

#[tauri::command]
pub fn list_detection_rules(
    project_id: Option<String>,
    vault: State<'_, VaultState>,
) -> Result<Vec<DetectionRule>, String> {
    vault.read(|opened| opened.repo.rules(scope(&project_id)))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectionOptions {
    pub language: String,
    pub session_rules: Vec<DetectionRule>,
    pub ignored_phrases: Vec<DetectionRule>,
    pub required_context: Vec<DetectionRule>,
}

impl Default for DetectionOptions {
    fn default() -> Self {
        Self {
            language: "en".into(),
            session_rules: vec![],
            ignored_phrases: vec![],
            required_context: vec![],
        }
    }
}

impl DetectionOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.language.as_str(), "en" | "fr" | "es" | "ar") {
            return Err("Choose English, French, Spanish, or Arabic".into());
        }
        for rules in [
            &self.session_rules,
            &self.ignored_phrases,
            &self.required_context,
        ] {
            if rules.len() > 200
                || rules.iter().any(|rule| {
                    rule.phrase.trim().is_empty()
                        || rule.phrase.len() > 500
                        || !crate::review::supported_entity(&rule.entity_type)
                })
            {
                return Err("Use up to 200 phrases per rule type, at most 500 UTF-8 bytes each, with valid categories".into());
            }
        }
        Ok(())
    }
}

#[tauri::command]
pub async fn detect_with_rules(
    app: tauri::AppHandle,
    state: State<'_, crate::engine::EngineState>,
    project_id: Option<String>,
    text: String,
    entities: Vec<String>,
    options: Option<DetectionOptions>,
    vault: State<'_, VaultState>,
) -> Result<serde_json::Value, String> {
    let options = options.unwrap_or_default();
    options.validate()?;
    let (rules, epoch) = {
        let session = vault.enter()?;
        match &session.opened {
            Some(opened) => (opened.repo.rules(scope(&project_id))?, Some(session.epoch)),
            None => (Vec::new(), None),
        }
    };
    let result = crate::engine::detect_local(app, state.inner().clone(), text, entities, rules, options).await;
    if let Some(epoch) = epoch {
        vault.enter()?.verify_epoch(epoch)?;
    }
    result
}

#[cfg(test)]
mod new_workflow_tests {
    #[test]
    fn detection_options_are_backward_compatible_and_bounded() {
        let defaults: super::DetectionOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults.language, "en");
        assert!(defaults.validate().is_ok());
        for language in ["en", "fr", "es", "ar"] {
            let options = super::DetectionOptions {
                language: language.into(),
                ..Default::default()
            };
            assert!(options.validate().is_ok());
        }
        let mut options = defaults;
        options.language = "unsupported".into();
        assert!(options.validate().is_err());
        options.language = "ar".into();
        options.ignored_phrases = vec![super::DetectionRule {
            phrase: "ع".repeat(251),
            entity_type: "PERSON".into(),
        }];
        assert!(options.validate().is_err());
        options.ignored_phrases = vec![
            super::DetectionRule {
                phrase: "Orange".into(),
                entity_type: "ORGANIZATION".into()
            };
            201
        ];
        assert!(options.validate().is_err());
    }

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

#[cfg(test)]
mod project_tests {
    use super::*;

    fn process(data: &mut SessionVault, project: &str) -> ProcessedRecord {
        data.update_project(Some(project), |scope| {
            scope.process(
                "Hiba Swaidan".into(),
                vec![ReviewedEntity {
                    id: "hit".into(),
                    entity_type: "PERSON".into(),
                    text: "Hiba Swaidan".into(),
                    start: 0,
                    end: 12,
                }],
                "pseudonymize",
            )
        })
        .unwrap()
    }

    #[test]
    fn projects_reuse_only_their_own_identities_and_keep_unique_tokens() {
        let mut data = SessionVault::default();
        let general = process(&mut data, "default");
        let first = data.create_project("Case A".into()).unwrap();
        let second = data.create_project("Case B".into()).unwrap();
        let a = process(&mut data, &first);
        let b = process(&mut data, &second);
        assert_eq!(a.text, process(&mut data, &first).text);
        assert_ne!(a.text, b.text);
        assert_ne!(general.text, a.text);
        assert_eq!(general.text, process(&mut data, "default").text);
        assert_eq!(data.project(Some(&first)).unwrap().pseudonyms.len(), 1);
        assert!(data.project(Some(&second)).unwrap().restore(&a.id).is_err());
        assert_eq!(
            data.project(Some(&first)).unwrap().restore(&a.id).unwrap(),
            "Hiba Swaidan"
        );
        let saved = serde_json::to_vec(&data).unwrap();
        let mut reopened: SessionVault = serde_json::from_slice(&saved).unwrap();
        assert_eq!(a.text, process(&mut reopened, &first).text);
        assert_eq!(b.text, process(&mut reopened, &second).text);
        assert!(reopened.project(Some("missing")).is_err());
    }

    #[test]
    fn projects_survive_encrypted_reopen_and_history_clear_is_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.vault");
        let password = "project test passphrase";
        let mut vault = UnlockedVault::open(path.clone(), password, true).unwrap();
        let (a, b) = vault
            .update(|data| {
                Ok((
                    data.create_project("Case A".into())?,
                    data.create_project("Case B".into())?,
                ))
            })
            .unwrap();
        let first = vault.update(|data| Ok(process(data, &a))).unwrap();
        let second = vault.update(|data| Ok(process(data, &b))).unwrap();
        let encrypted = std::fs::read_to_string(&path).unwrap();
        assert!(!encrypted.contains("Hiba"));
        assert!(!encrypted.contains("Case A"));
        drop(vault);
        let mut reopened = UnlockedVault::open(path, password, false).unwrap();
        assert_eq!(
            first.text,
            reopened.update(|data| Ok(process(data, &a))).unwrap().text
        );
        reopened
            .update(|data| {
                data.update_project(Some(&a), |scope| {
                    scope.records.clear();
                    Ok(())
                })
            })
            .unwrap();
        assert!(reopened.data.project(Some(&a)).unwrap().records.is_empty());
        assert_eq!(reopened.data.project(Some(&a)).unwrap().pseudonyms.len(), 1);
        assert_eq!(
            reopened
                .data
                .project(Some(&b))
                .unwrap()
                .restore(&second.id)
                .unwrap(),
            "Hiba Swaidan"
        );
        assert_eq!(
            first.text,
            reopened.update(|data| Ok(process(data, &a))).unwrap().text
        );
    }

    #[test]
    fn deleting_mapping_is_project_scoped_persisted_and_never_reuses_tokens() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("deletion.vault");
        let password = "mapping deletion passphrase";
        let mut vault = UnlockedVault::open(path.clone(), password, true).unwrap();
        let a = vault
            .update(|data| data.create_project("Project A".into()))
            .unwrap();
        let b = vault
            .update(|data| data.create_project("Project B".into()))
            .unwrap();
        let first = vault.update(|data| Ok(process(data, &a))).unwrap();
        let other = vault.update(|data| Ok(process(data, &b))).unwrap();
        let identity = vault.data.project(Some(&a)).unwrap().pseudonyms[0]
            .id
            .clone();
        let before = serde_json::to_value(&vault.data).unwrap();
        assert!(
            vault
                .update(|data| data
                    .update_project(Some(&b), |project| project.delete_identity(&identity)))
                .is_err()
        );
        assert_eq!(before, serde_json::to_value(&vault.data).unwrap());
        vault
            .update(|data| {
                data.update_project(Some(&a), |project| project.delete_identity(&identity))
            })
            .unwrap();
        drop(vault);
        let mut reopened = UnlockedVault::open(path, password, false).unwrap();
        assert!(
            reopened
                .data
                .project(Some(&a))
                .unwrap()
                .pseudonyms
                .is_empty()
        );
        assert_eq!(
            reopened
                .data
                .project(Some(&a))
                .unwrap()
                .restore(&first.id)
                .unwrap(),
            "Hiba Swaidan"
        );
        assert_eq!(
            other.text,
            reopened.update(|data| Ok(process(data, &b))).unwrap().text
        );
        let fresh = reopened.update(|data| Ok(process(data, &a))).unwrap();
        assert_ne!(fresh.text, first.text);
        assert_ne!(fresh.text, other.text);
        assert!(
            reopened
                .update(|data| data
                    .update_project(Some(&a), |project| project.delete_identity(&identity)))
                .is_err()
        );
        let general = reopened
            .update(|data| data.save_identity("PERSON", "John Smith", ""))
            .unwrap();
        reopened
            .update(|data| {
                data.update_project(None, |project| project.delete_identity(&general.id))
            })
            .unwrap();
        let new_general = reopened
            .update(|data| data.save_identity("PERSON", "John Smith", ""))
            .unwrap();
        assert_ne!(general.pseudonym, new_general.pseudonym);
    }

    #[test]
    fn normalized_lookup_keeps_abbreviations_and_categories_separate_after_reopen() {
        let mut data = SessionVault::default();
        let a = data.create_project("A".into()).unwrap();
        let b = data.create_project("B".into()).unwrap();
        let mut save = |project: &str, category: &str, value: &str| {
            data.update_project(Some(project), |scope| {
                scope.save_identity(category, value, "")
            })
            .unwrap()
        };
        let original = save(&a, "PERSON", "Hiba Swaidan");
        for name in ["hiba swaidan", "HIBA SWAIDAN", " Hiba   Swaidan "] {
            assert_eq!(save(&a, "PERSON", name).id, original.id);
        }
        for name in ["Hiba", "H. Swaidan"] {
            assert_ne!(save(&a, "PERSON", name).id, original.id);
        }
        assert_ne!(save(&a, "ORGANIZATION", "Hiba Swaidan").id, original.id);
        assert_ne!(
            save(&b, "PERSON", "Hiba Swaidan").pseudonym,
            original.pseudonym
        );
        let encoded = serde_json::to_string(&data).unwrap();
        assert!(!encoded.contains("identity_index"));
        assert!(!encoded.contains("project_index"));
        let mut reopened: SessionVault = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            reopened
                .update_project(Some(&a), |scope| scope.save_identity(
                    "PERSON",
                    "HIBA SWAIDAN",
                    ""
                ))
                .unwrap()
                .id,
            original.id
        );
        // Removing a middle entry must not leave cached vector positions stale.
        reopened
            .update_project(Some(&a), |scope| scope.delete_identity(&original.id))
            .unwrap();
        let short = reopened
            .update_project(Some(&a), |scope| scope.save_identity("PERSON", "Hiba", ""))
            .unwrap();
        assert_eq!(short.original, "Hiba");
        assert_eq!(reopened.project(Some(&a)).unwrap().pseudonyms.len(), 3);
    }

    #[test]
    fn project_deletion_requires_confirmation_persists_and_reserves_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project-delete.vault");
        let password = "project deletion test password";
        let mut vault = UnlockedVault::open(path.clone(), password, true).unwrap();
        let a = vault
            .update(|data| data.create_project("Case A".into()))
            .unwrap();
        let b = vault
            .update(|data| data.create_project("Case B".into()))
            .unwrap();
        let first = vault.update(|data| Ok(process(data, &a))).unwrap();
        let other = vault.update(|data| Ok(process(data, &b))).unwrap();
        let before = serde_json::to_value(&vault.data).unwrap();
        assert!(
            vault
                .update(|data| data.delete_project(&a, "wrong"))
                .is_err()
        );
        assert!(
            vault
                .update(|data| data.delete_project("default", "General"))
                .is_err()
        );
        assert_eq!(before, serde_json::to_value(&vault.data).unwrap());
        vault
            .update(|data| data.delete_project(&a, "Case A"))
            .unwrap();
        drop(vault);
        let mut reopened = UnlockedVault::open(path, password, false).unwrap();
        assert!(reopened.data.project(Some(&a)).is_err());
        assert_eq!(
            reopened
                .data
                .project(Some(&b))
                .unwrap()
                .restore(&other.id)
                .unwrap(),
            "Hiba Swaidan"
        );
        let c = reopened
            .update(|data| data.create_project("Case A".into()))
            .unwrap();
        assert_ne!(c, a);
        let fresh = reopened.update(|data| Ok(process(data, &c))).unwrap();
        assert_ne!(fresh.text, first.text);
        assert_ne!(fresh.text, other.text);
        assert!(
            reopened
                .update(|data| data.delete_project(&a, "Case A"))
                .is_err()
        );
    }

    #[test]
    fn legacy_vaults_keep_mappings_in_general_and_names_are_validated() {
        let mut legacy = SessionVault::default();
        let old = process(&mut legacy, "default");
        let mut json = serde_json::to_value(&legacy).unwrap();
        json.as_object_mut().unwrap().remove("projects");
        let mut upgraded: SessionVault = serde_json::from_value(json).unwrap();
        assert_eq!(old.text, process(&mut upgraded, "default").text);
        assert!(upgraded.create_project(" ".into()).is_err());
        assert!(upgraded.create_project("General".into()).is_err());
        upgraded.create_project("Case".into()).unwrap();
        assert!(upgraded.create_project(" case ".into()).is_err());
        let before = serde_json::to_value(&upgraded).unwrap();
        let output = upgraded
            .project(Some("default"))
            .unwrap()
            .clone()
            .process(
                "Hiba".into(),
                vec![ReviewedEntity {
                    id: "1".into(),
                    entity_type: "PERSON".into(),
                    text: "Hiba".into(),
                    start: 0,
                    end: 4,
                }],
                "anonymize",
            )
            .unwrap();
        assert_eq!(output.text, "PERSON_001");
        assert_eq!(before, serde_json::to_value(&upgraded).unwrap());
    }
}

#[cfg(test)]
mod vault_session_tests {
    use super::VaultSession;

    #[test]
    fn an_action_cannot_continue_after_an_epoch_transition() {
        let mut session = VaultSession::default();
        let in_flight = session.epoch;
        assert!(session.verify_epoch(in_flight).is_ok());
        session.advance().unwrap();
        assert!(session.verify_epoch(in_flight).is_err());
    }
}
