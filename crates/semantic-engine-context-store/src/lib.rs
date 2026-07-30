use std::{error::Error, fmt, path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use semantic_engine_core::AnswerTarget;
use semantic_engine_package::{ImportedContext, SourceMetadata};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StoredContext {
    pub name: String,
    pub package_id: String,
    pub version: String,
    pub license: String,
    pub locales: Vec<String>,
    pub sources: Vec<SourceMetadata>,
    pub target_count: usize,
    pub package_sha256: String,
    pub targets_sha256: String,
    pub targets: Vec<AnswerTarget>,
}

impl StoredContext {
    fn from_imported(context: &ImportedContext) -> Self {
        Self {
            name: context.name.clone(),
            package_id: context.id.clone(),
            version: context.version.to_string(),
            license: context.spdx_license_expression.clone(),
            locales: context.locales.clone(),
            sources: context.sources.clone(),
            target_count: context.targets.len(),
            package_sha256: context.package_sha256.clone(),
            targets_sha256: context.targets_sha256.clone(),
            targets: context.targets.clone(),
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    ImmutableVersionConflict { package_id: String, version: String },
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::Json(error) => write!(formatter, "stored context JSON error: {error}"),
            Self::ImmutableVersionConflict { package_id, version } => write!(
                formatter,
                "context version is immutable: {package_id} version {version} already has different bytes"
            ),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::ImmutableVersionConflict { .. } => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub struct ContextStore {
    connection: Connection,
}

impl ContextStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS context_versions (
                package_sha256 TEXT PRIMARY KEY NOT NULL,
                package_id TEXT NOT NULL,
                version TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                UNIQUE(package_id, version)
            );
            CREATE TABLE IF NOT EXISTS activation_history (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                package_sha256 TEXT NOT NULL REFERENCES context_versions(package_sha256),
                previous_sequence INTEGER REFERENCES activation_history(sequence)
            );
            CREATE TABLE IF NOT EXISTS context_state (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                active_sequence INTEGER REFERENCES activation_history(sequence)
            );
            INSERT OR IGNORE INTO context_state(singleton, active_sequence) VALUES (1, NULL);
            ",
        )?;
        Ok(Self { connection })
    }

    pub fn activate(&mut self, imported: &ImportedContext) -> Result<StoredContext, StoreError> {
        let context = StoredContext::from_imported(imported);
        let payload = serde_json::to_string(&context)?;
        let transaction =
            self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = current_pointer(&transaction)?;

        if current.as_ref().is_some_and(|(_, hash)| hash == &context.package_sha256) {
            transaction.commit()?;
            return Ok(context);
        }

        transaction.execute(
            "INSERT OR IGNORE INTO context_versions
             (package_sha256, package_id, version, payload_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![context.package_sha256, context.package_id, context.version, payload],
        )?;

        let stored_identity: Option<(String, String)> = transaction
            .query_row(
                "SELECT package_sha256, payload_json FROM context_versions
                 WHERE package_id = ?1 AND version = ?2",
                params![context.package_id, context.version],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if stored_identity.as_ref() != Some(&(context.package_sha256.clone(), payload.clone())) {
            return Err(StoreError::ImmutableVersionConflict {
                package_id: context.package_id.clone(),
                version: context.version.clone(),
            });
        }

        transaction.execute(
            "INSERT INTO activation_history(package_sha256, previous_sequence)
             VALUES (?1, ?2)",
            params![context.package_sha256, current.map(|(sequence, _)| sequence)],
        )?;
        let sequence = transaction.last_insert_rowid();
        transaction.execute(
            "UPDATE context_state SET active_sequence = ?1 WHERE singleton = 1",
            [sequence],
        )?;
        transaction.commit()?;
        Ok(context)
    }

    pub fn current(&self) -> Result<Option<StoredContext>, StoreError> {
        let payload = self
            .connection
            .query_row(
                "SELECT versions.payload_json
                 FROM context_state AS state
                 JOIN activation_history AS history ON history.sequence = state.active_sequence
                 JOIN context_versions AS versions
                   ON versions.package_sha256 = history.package_sha256
                 WHERE state.singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        payload.map(|payload| serde_json::from_str(&payload).map_err(StoreError::from)).transpose()
    }

    pub fn rollback(&mut self) -> Result<Option<StoredContext>, StoreError> {
        let transaction =
            self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous_sequence = transaction
            .query_row(
                "SELECT history.previous_sequence
                 FROM context_state AS state
                 JOIN activation_history AS history ON history.sequence = state.active_sequence
                 WHERE state.singleton = 1",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();

        let Some(previous_sequence) = previous_sequence else {
            transaction.commit()?;
            return Ok(None);
        };

        transaction.execute(
            "UPDATE context_state SET active_sequence = ?1 WHERE singleton = 1",
            [previous_sequence],
        )?;
        let restored = context_for_sequence(&transaction, previous_sequence)?;
        transaction.commit()?;
        Ok(Some(restored))
    }
}

fn current_pointer(connection: &Connection) -> Result<Option<(i64, String)>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT history.sequence, history.package_sha256
             FROM context_state AS state
             JOIN activation_history AS history ON history.sequence = state.active_sequence
             WHERE state.singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
}

fn context_for_sequence(
    connection: &Connection,
    sequence: i64,
) -> Result<StoredContext, StoreError> {
    let payload = connection.query_row(
        "SELECT versions.payload_json
         FROM activation_history AS history
         JOIN context_versions AS versions ON versions.package_sha256 = history.package_sha256
         WHERE history.sequence = ?1",
        [sequence],
        |row| row.get::<_, String>(0),
    )?;
    Ok(serde_json::from_str(&payload)?)
}
