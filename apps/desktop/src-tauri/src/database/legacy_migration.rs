use super::{DbResult, Result, read_integer, repository::insert_rules};
use crate::protection::{self, SessionVault};
use rusqlite::{Connection, TransactionBehavior, params};
use std::collections::HashSet;

const INVALID: &str =
    "Legacy Vault data violates a migration invariant. The recovery data was preserved.";

pub(super) fn import(connection: &mut Connection, data: &SessionVault) -> Result<()> {
    if data.projects.len() > 100 {
        return Err(INVALID.into());
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .vault()?;
    for (category, counter) in &data.counters {
        if !crate::review::supported_entity(category) {
            return Err(INVALID.into());
        }
        tx.execute(
            "INSERT INTO pseudonym_counters VALUES (?1,?2)",
            params![category, integer(*counter)?],
        )
        .map_err(|_| INVALID)?;
    }

    insert_scope(&tx, "default", data, data)?;
    for (offset, project) in data.projects.iter().enumerate() {
        check_identifier(&project.id)?;
        if !project.data.projects.is_empty() || project.name.trim() != project.name {
            return Err(INVALID.into());
        }
        let key = protection::project_name_key(&project.name).map_err(|_| INVALID)?;
        tx.execute(
            "INSERT INTO projects VALUES (?1,?2,?3,?4)",
            params![project.id, project.name, key, integer(offset + 1)?],
        )
        .map_err(|_| INVALID)?;
        insert_scope(&tx, &project.id, &project.data, data)?;
    }
    tx.commit().vault()
}

fn insert_scope(
    connection: &Connection,
    id: &str,
    scope: &SessionVault,
    root: &SessionVault,
) -> Result<()> {
    if scope.records.len() > 100 {
        return Err(INVALID.into());
    }
    protection::validate_rules(&scope.rules).map_err(|_| INVALID)?;
    insert_rules(connection, id, &scope.rules)?;
    let mut mapping = connection
        .prepare("INSERT INTO pseudonym_mappings VALUES (?1,?2,?3,?4,?5,?6,?7)")
        .vault()?;
    for (position, value) in scope.pseudonyms.iter().enumerate() {
        check_identifier(&value.id)?;
        let (original, normalized) =
            protection::mapping_value(&value.entity_type, &value.original).map_err(|_| INVALID)?;
        let prefix = protection::category_prefix(&value.entity_type).ok_or(INVALID)?;
        let suffix = value
            .pseudonym
            .strip_prefix(prefix)
            .and_then(|token| token.strip_prefix('_'))
            .and_then(|digits| digits.parse::<i64>().ok())
            .ok_or(INVALID)?;
        let counter = integer(*root.counters.get(&value.entity_type).ok_or(INVALID)?)?;
        if original != value.original
            || suffix < 1
            || suffix > counter
            || value.pseudonym != format!("{prefix}_{suffix:03}")
        {
            return Err(INVALID.into());
        }
        mapping
            .execute(params![
                value.id,
                id,
                value.entity_type,
                value.original,
                normalized,
                value.pseudonym,
                integer(position)?
            ])
            .map_err(|_| INVALID)?;
    }
    let mut history = connection
        .prepare("INSERT INTO processing_history VALUES (?1,?2,?3,?4,?5,?6,?7)")
        .vault()?;
    for (position, stored) in scope.records.iter().enumerate() {
        let record = &stored.public;
        check_identifier(&record.id)?;
        history
            .execute(params![
                record.id,
                id,
                record.mode,
                record.text,
                stored.original,
                integer(record.count)?,
                integer(position)?
            ])
            .map_err(|_| INVALID)?;
    }
    Ok(())
}

fn integer(value: usize) -> Result<i64> {
    i64::try_from(value).map_err(|_| INVALID.into())
}

fn check_identifier(id: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(INVALID.into());
    }
    Ok(())
}

pub(super) fn verify(connection: &Connection, data: &SessionVault) -> Result<()> {
    let mut scopes = vec![("default", "General", data)];
    scopes.extend(
        data.projects
            .iter()
            .map(|project| (project.id.as_str(), project.name.as_str(), &project.data)),
    );
    let mut projects = connection
        .prepare("SELECT project_id,name,position FROM projects ORDER BY position")
        .vault()?;
    let rows = projects
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                read_integer(r, 2)?,
            ))
        })
        .vault()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .vault()?;
    if rows.len() != scopes.len() {
        return Err(INVALID.into());
    }
    let mut known = HashSet::new();
    for (position, ((id, name, scope), row)) in scopes.into_iter().zip(rows).enumerate() {
        if row != (id.to_string(), name.to_string(), position) || !known.insert(id) {
            return Err(INVALID.into());
        }
        verify_scope(connection, id, scope)?;
    }
    let mut counters = connection
        .prepare("SELECT category_code,high_water FROM pseudonym_counters")
        .vault()?;
    let rows = counters
        .query_map([], |r| Ok((r.get::<_, String>(0)?, read_integer(r, 1)?)))
        .vault()?
        .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
        .vault()?;
    if rows != data.counters {
        return Err(INVALID.into());
    }
    Ok(())
}

fn verify_scope(conn: &Connection, project: &str, data: &SessionVault) -> Result<()> {
    let mut mappings = conn
        .prepare(
            "SELECT mapping_id,category_code,original_value,normalized_value,pseudonym,position
        FROM pseudonym_mappings WHERE project_id=?1 ORDER BY position",
        )
        .vault()?;
    let rows = mappings
        .query_map([project], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                read_integer(r, 5)?,
            ))
        })
        .vault()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .vault()?;
    if rows.len() != data.pseudonyms.len() {
        return Err(INVALID.into());
    }
    for (position, ((id, category, original, normalized, token, stored), expected)) in
        rows.into_iter().zip(&data.pseudonyms).enumerate()
    {
        if id != expected.id
            || category != expected.entity_type
            || original != expected.original
            || token != expected.pseudonym
            || normalized != protection::identity_key(&category, &original)
            || position != stored
        {
            return Err(INVALID.into());
        }
    }

    let mut history = conn
        .prepare(
            "SELECT record_id,mode,processed_text,original_text,replacement_count,position
        FROM processing_history WHERE project_id=?1 ORDER BY position",
        )
        .vault()?;
    let rows = history
        .query_map([project], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                read_integer(r, 4)?,
                read_integer(r, 5)?,
            ))
        })
        .vault()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .vault()?;
    if rows.len() != data.records.len() {
        return Err(INVALID.into());
    }
    for (position, ((id, mode, text, original, count, stored), expected)) in
        rows.into_iter().zip(&data.records).enumerate()
    {
        let record = &expected.public;
        if id != record.id
            || mode != record.mode
            || text != record.text
            || original != expected.original
            || count != record.count
            || position != stored
        {
            return Err(INVALID.into());
        }
    }

    let mut rules = conn
        .prepare(
            "SELECT phrase,category_code,position FROM detection_rules
        WHERE project_id=?1 ORDER BY position",
        )
        .vault()?;
    let rows = rules
        .query_map([project], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                read_integer(r, 2)?,
            ))
        })
        .vault()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .vault()?;
    if rows.len() != data.rules.len() {
        return Err(INVALID.into());
    }
    for (position, ((phrase, category, stored), expected)) in
        rows.into_iter().zip(&data.rules).enumerate()
    {
        if phrase != expected.phrase || category != expected.entity_type || position != stored {
            return Err(INVALID.into());
        }
    }
    Ok(())
}
