use super::{DbResult, Result, read_integer, write_integer};
use crate::protection::{self, DetectionRule, ProcessedRecord, ProjectSummary, SavedIdentity};
use crate::review::ReviewedEntity;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

pub struct Repository(pub(super) Connection);

impl Repository {
    pub fn projects(&self) -> Result<Vec<ProjectSummary>> {
        let mut stmt = self
            .0
            .prepare(
                "SELECT p.project_id, p.name,
                    (SELECT count(*) FROM processing_history h WHERE h.project_id=p.project_id),
                    (SELECT count(*) FROM pseudonym_mappings m WHERE m.project_id=p.project_id)
             FROM projects p ORDER BY p.position",
            )
            .vault()?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ProjectSummary {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    documents: read_integer(r, 2)?,
                    identities: read_integer(r, 3)?,
                })
            })
            .vault()?;
        rows.collect::<rusqlite::Result<_>>().vault()
    }

    pub fn create_project(&mut self, name: String) -> Result<String> {
        let name = name.trim();
        let key = protection::project_name_key(name)?;
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .vault()?;
        if tx
            .query_row("SELECT count(*) FROM projects", [], |r| r.get::<_, i64>(0))
            .vault()?
            > 100
        {
            return Err("The Vault supports up to 100 projects".into());
        }
        if tx
            .query_row("SELECT 1 FROM projects WHERE name_key=?1", [&key], |_| {
                Ok(())
            })
            .optional()
            .vault()?
            .is_some()
        {
            return Err("A project with this name already exists".into());
        }
        let last: i64 = tx
            .query_row("SELECT max(position) FROM projects", [], |r| r.get(0))
            .vault()?;
        let position = last.checked_add(1).ok_or("Vault ordering is exhausted")?;
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO projects VALUES (?1,?2,?3,?4)",
            params![id, name, key, position],
        )
        .vault()?;
        tx.commit().vault()?;
        Ok(id)
    }

    pub fn delete_project(&mut self, project: &str, confirmation: &str) -> Result<()> {
        if project == "default" {
            return Err("The General project cannot be deleted".into());
        }
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .vault()?;
        let name: String = tx
            .query_row(
                "SELECT name FROM projects WHERE project_id=?1",
                [project],
                |r| r.get(0),
            )
            .optional()
            .vault()?
            .ok_or("Project not found. Select an existing project.")?;
        if name != confirmation {
            return Err("Type the project name exactly to confirm deletion".into());
        }
        tx.execute("DELETE FROM projects WHERE project_id=?1", [project])
            .vault()?;
        tx.commit().vault()
    }

    pub fn mappings(&self, project: &str) -> Result<Vec<SavedIdentity>> {
        require_project(&self.0, project)?;
        let mut stmt = self
            .0
            .prepare(
                "SELECT mapping_id, category_code, original_value, pseudonym
            FROM pseudonym_mappings WHERE project_id=?1 ORDER BY position",
            )
            .vault()?;
        let rows = stmt.query_map([project], mapping_row).vault()?;
        rows.collect::<rusqlite::Result<_>>().vault()
    }

    pub fn mapping(&self, project: &str, id: &str) -> Result<SavedIdentity> {
        require_project(&self.0, project)?;
        self.0
            .query_row(
                "SELECT mapping_id,category_code,original_value,pseudonym
            FROM pseudonym_mappings WHERE project_id=?1 AND mapping_id=?2",
                [project, id],
                mapping_row,
            )
            .optional()
            .vault()?
            .ok_or_else(|| "Saved identity not found".into())
    }

    pub fn save_mapping(
        &mut self,
        project: &str,
        category: &str,
        original: &str,
    ) -> Result<SavedIdentity> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .vault()?;
        require_project(&tx, project)?;
        let identity = save_mapping(&tx, project, category, original, "")?;
        tx.commit().vault()?;
        Ok(identity)
    }

    pub fn delete_mapping(&mut self, project: &str, id: &str) -> Result<()> {
        require_project(&self.0, project)?;
        if self
            .0
            .execute(
                "DELETE FROM pseudonym_mappings WHERE project_id=?1 AND mapping_id=?2",
                [project, id],
            )
            .vault()?
            == 0
        {
            return Err("Saved mapping not found in this project".into());
        }
        Ok(())
    }

    pub fn process(
        &mut self,
        project: &str,
        text: String,
        approved: Vec<ReviewedEntity>,
        operation_id: Option<uuid::Uuid>,
    ) -> Result<ProcessedRecord> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .vault()?;
        require_project(&tx, project)?;
        let operation_id = operation_id.map(|id| id.to_string());
        if let Some(id) = &operation_id {
            if tx
                .query_row(
                    "SELECT 1 FROM processing_history WHERE record_id=?1",
                    [id],
                    |_| Ok(()),
                )
                .optional()
                .vault()?
                .is_some()
            {
                return Err("This processing request was already saved".into());
            }
        }
        if tx
            .query_row(
                "SELECT count(*) FROM processing_history WHERE project_id=?1",
                [project],
                |r| r.get::<_, i64>(0),
            )
            .vault()?
            >= 100
        {
            return Err("Document history is full. Clear history in the Vault; saved identities will be kept.".into());
        }
        let mut record =
            protection::render_processed(&text, &approved, "pseudonymize", |entity| {
                save_mapping(&tx, project, &entity.entity_type, &entity.text, &text)
                    .map(|identity| identity.pseudonym)
            })?;
        if let Some(id) = operation_id {
            record.id = id;
        }
        let position = next_position(
            &tx,
            "SELECT max(position) FROM processing_history WHERE project_id=?1",
            project,
        )?;
        tx.execute(
            "INSERT INTO processing_history VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                record.id,
                project,
                record.mode,
                record.text,
                text,
                write_integer(record.count)?,
                position
            ],
        )
        .vault()?;
        tx.commit().vault()?;
        Ok(record)
    }

    pub fn records(&self, project: &str) -> Result<Vec<ProcessedRecord>> {
        require_project(&self.0, project)?;
        let mut stmt = self
            .0
            .prepare(
                "SELECT record_id,mode,processed_text,replacement_count
            FROM processing_history WHERE project_id=?1 ORDER BY position DESC",
            )
            .vault()?;
        let rows = stmt
            .query_map([project], |r| {
                Ok(ProcessedRecord {
                    id: r.get(0)?,
                    mode: r.get(1)?,
                    text: r.get(2)?,
                    count: read_integer(r, 3)?,
                    export_replacements: Vec::new(),
                })
            })
            .vault()?;
        rows.collect::<rusqlite::Result<_>>().vault()
    }

    pub fn restore(&self, project: &str, id: &str) -> Result<String> {
        require_project(&self.0, project)?;
        self.0
            .query_row(
                "SELECT original_text FROM processing_history WHERE project_id=?1 AND record_id=?2",
                [project, id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .vault()?
            .ok_or("The record is no longer in the vault")?
            .ok_or_else(|| "Anonymized records cannot be restored".into())
    }

    pub fn clear_history(&mut self, project: &str) -> Result<()> {
        require_project(&self.0, project)?;
        self.0
            .execute(
                "DELETE FROM processing_history WHERE project_id=?1",
                [project],
            )
            .vault()?;
        Ok(())
    }

    pub fn rules(&self, project: &str) -> Result<Vec<DetectionRule>> {
        require_project(&self.0, project)?;
        let mut stmt = self.0.prepare("SELECT phrase,category_code FROM detection_rules WHERE project_id=?1 ORDER BY position").vault()?;
        let rows = stmt
            .query_map([project], |r| {
                Ok(DetectionRule {
                    phrase: r.get(0)?,
                    entity_type: r.get(1)?,
                })
            })
            .vault()?;
        rows.collect::<rusqlite::Result<_>>().vault()
    }

    pub fn replace_rules(&mut self, project: &str, rules: &[DetectionRule]) -> Result<()> {
        protection::validate_rules(rules)?;
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .vault()?;
        require_project(&tx, project)?;
        tx.execute("DELETE FROM detection_rules WHERE project_id=?1", [project])
            .vault()?;
        insert_rules(&tx, project, rules)?;
        tx.commit().vault()
    }

    pub fn reset(&mut self) -> Result<()> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .vault()?;
        tx.execute_batch(
            "DELETE FROM projects WHERE project_id <> 'default';
            DELETE FROM pseudonym_mappings; DELETE FROM processing_history;
            DELETE FROM detection_rules; DELETE FROM pseudonym_counters;",
        )
        .vault()?;
        tx.commit().vault()
    }
}

pub(super) fn insert_rules(
    conn: &Connection,
    project: &str,
    rules: &[DetectionRule],
) -> Result<()> {
    let mut stmt = conn
        .prepare("INSERT INTO detection_rules VALUES (?1,?2,?3,?4)")
        .vault()?;
    for (position, rule) in rules.iter().enumerate() {
        stmt.execute(params![
            project,
            write_integer(position)?,
            rule.phrase,
            rule.entity_type
        ])
        .vault()?;
    }
    Ok(())
}

fn require_project(conn: &Connection, project: &str) -> Result<()> {
    conn.query_row(
        "SELECT 1 FROM projects WHERE project_id=?1",
        [project],
        |_| Ok(()),
    )
    .optional()
    .vault()?
    .ok_or_else(|| "Project not found. Select an existing project.".into())
}

fn next_position(conn: &Connection, sql: &str, project: &str) -> Result<i64> {
    conn.query_row(sql, [project], |r| r.get::<_, Option<i64>>(0))
        .vault()?
        .unwrap_or(-1)
        .checked_add(1)
        .ok_or_else(|| "Vault ordering is exhausted".into())
}

fn mapping_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SavedIdentity> {
    Ok(SavedIdentity {
        id: r.get(0)?,
        entity_type: r.get(1)?,
        original: r.get(2)?,
        pseudonym: r.get(3)?,
    })
}

fn save_mapping(
    conn: &Connection,
    project: &str,
    category: &str,
    value: &str,
    source: &str,
) -> Result<SavedIdentity> {
    let (original, normalized) = protection::mapping_value(category, value)?;
    if let Some(identity) = conn
        .query_row(
            "SELECT mapping_id,category_code,original_value,pseudonym
        FROM pseudonym_mappings WHERE project_id=?1 AND category_code=?2 AND normalized_value=?3",
            params![project, category, normalized],
            mapping_row,
        )
        .optional()
        .vault()?
    {
        return Ok(identity);
    }
    let prefix: String = conn
        .query_row(
            "SELECT token_prefix FROM pii_categories WHERE code=?1",
            [category],
            |r| r.get(0),
        )
        .vault()?;
    let counter = conn
        .query_row(
            "SELECT high_water FROM pseudonym_counters WHERE category_code=?1",
            [category],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .vault()?
        .unwrap_or_default();
    let (counter, pseudonym) = protection::next_token(counter, &prefix, source)?;
    let identity = SavedIdentity {
        id: uuid::Uuid::new_v4().to_string(),
        entity_type: category.into(),
        original: original.into(),
        pseudonym,
    };
    let position = next_position(
        conn,
        "SELECT max(position) FROM pseudonym_mappings WHERE project_id=?1",
        project,
    )?;
    conn.execute(
        "INSERT INTO pseudonym_counters VALUES (?1,?2)
        ON CONFLICT(category_code) DO UPDATE SET high_water=excluded.high_water",
        params![category, counter],
    )
    .vault()?;
    conn.execute(
        "INSERT INTO pseudonym_mappings VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            identity.id,
            project,
            category,
            original,
            normalized,
            identity.pseudonym,
            position
        ],
    )
    .vault()?;
    Ok(identity)
}
