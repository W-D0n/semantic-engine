use std::{
    fmt,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};
use semantic_engine_core::{
    Decision, EvidenceKind, MAX_IDENTIFIER_CHARS, MAX_RESOLUTION_NOTE_CHARS, MAX_TARGETS_PER_ROUND,
    OperatorResolution, Validation, ValidationIssue,
};
use serde::{Deserialize, Serialize};

const MAX_RETAINED_VALIDATIONS: usize = 1_000_000;
const MAX_RETENTION_AGE_SECONDS: u64 = 10 * 365 * 24 * 60 * 60;
const MAX_AUDIT_PAGE: usize = 1_000;
pub const AUDIT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub max_validations: usize,
    pub max_age_seconds: Option<u64>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self { max_validations: 10_000, max_age_seconds: Some(30 * 24 * 60 * 60) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditValidation {
    pub sequence: u64,
    pub recorded_at_ms: u64,
    pub round_id: String,
    pub message_id: String,
    pub participant_id: String,
    pub source_sequence: u64,
    pub context_package_sha256: Option<String>,
    pub decision: Decision,
    pub target_id: Option<String>,
    pub score: f64,
    pub evidence_kinds: Vec<EvidenceKind>,
    pub issue: Option<ValidationIssue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditResolution {
    pub recorded_at_ms: u64,
    pub original_decision: Decision,
    pub final_decision: Decision,
    pub target_id: Option<String>,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub schema_version: u32,
    pub validation: AuditValidation,
    pub resolution: Option<AuditResolution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditError {
    Database(String),
    Conflict,
    MissingValidation,
    InvalidRecord,
    InvalidRetention,
}

impl fmt::Display for AuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(message) => write!(formatter, "audit database error: {message}"),
            Self::Conflict => write!(formatter, "audit identity already has different content"),
            Self::MissingValidation => write!(formatter, "audit validation does not exist"),
            Self::InvalidRecord => write!(formatter, "audit record is invalid"),
            Self::InvalidRetention => write!(formatter, "audit retention policy is invalid"),
        }
    }
}

impl std::error::Error for AuditError {}

impl From<rusqlite::Error> for AuditError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StoredValidation {
    round_id: String,
    message_id: String,
    participant_id: String,
    source_sequence: u64,
    context_package_sha256: Option<String>,
    decision: Decision,
    target_id: Option<String>,
    score: f64,
    evidence_kinds: Vec<EvidenceKind>,
    issue: Option<ValidationIssue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StoredResolution {
    original_decision: Decision,
    final_decision: Decision,
    target_id: Option<String>,
    note: String,
}

type StoredRow = (i64, i64, String, Option<i64>, Option<String>);

pub struct AuditStore {
    connection: Connection,
    retention: RetentionPolicy,
}

impl AuditStore {
    pub fn open(path: impl AsRef<Path>, retention: RetentionPolicy) -> Result<Self, AuditError> {
        validate_retention(&retention)?;
        let connection = Connection::open(path)?;
        Self::initialize(connection, retention)
    }

    pub fn open_in_memory(retention: RetentionPolicy) -> Result<Self, AuditError> {
        validate_retention(&retention)?;
        let connection = Connection::open_in_memory()?;
        Self::initialize(connection, retention)
    }

    fn initialize(connection: Connection, retention: RetentionPolicy) -> Result<Self, AuditError> {
        connection.busy_timeout(Duration::from_secs(2))?;
        let schema_version =
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?;
        if schema_version > AUDIT_SCHEMA_VERSION {
            return Err(AuditError::Database(format!(
                "audit schema version {schema_version} is newer than supported version {AUDIT_SCHEMA_VERSION}"
            )));
        }
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA secure_delete = ON;
             CREATE TABLE IF NOT EXISTS audit_validations (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 round_id TEXT NOT NULL,
                 message_id TEXT NOT NULL,
                 source_sequence INTEGER NOT NULL,
                 recorded_at_ms INTEGER NOT NULL,
                 payload_json TEXT NOT NULL,
                 UNIQUE(round_id, message_id)
             );
             CREATE INDEX IF NOT EXISTS audit_validations_round_order
                 ON audit_validations(round_id, source_sequence, sequence);
             CREATE TABLE IF NOT EXISTS audit_resolutions (
                 validation_sequence INTEGER PRIMARY KEY
                     REFERENCES audit_validations(sequence) ON DELETE CASCADE,
                 recorded_at_ms INTEGER NOT NULL,
                 payload_json TEXT NOT NULL
             );
             PRAGMA user_version = 1;",
        )?;
        Ok(Self { connection, retention })
    }

    pub fn record_validation(
        &mut self,
        validation: &Validation,
        context_package_sha256: Option<&str>,
    ) -> Result<AuditEntry, AuditError> {
        validate_validation(validation, context_package_sha256)?;
        let payload = StoredValidation {
            round_id: validation.round_id.clone(),
            message_id: validation.message_id.clone(),
            participant_id: validation.participant_id.clone(),
            source_sequence: validation.source_sequence,
            context_package_sha256: context_package_sha256.map(str::to_owned),
            decision: validation.decision.clone(),
            target_id: validation.target_id.clone(),
            score: validation.score,
            evidence_kinds: validation.evidence.iter().map(|item| item.kind.clone()).collect(),
            issue: validation.issue.clone(),
        };
        let payload_json = serde_json::to_string(&payload).map_err(database_serialization)?;
        let recorded_at_ms = now_ms()?;
        let transaction = self.connection.transaction()?;

        let existing = transaction
            .query_row(
                "SELECT sequence, payload_json
                 FROM audit_validations
                 WHERE round_id = ?1 AND message_id = ?2",
                params![validation.round_id, validation.message_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((sequence, existing_json)) = existing {
            if existing_json != payload_json {
                return Err(AuditError::Conflict);
            }
            let entry = load_entry(&transaction, sequence)?.ok_or_else(missing_database_row)?;
            transaction.commit()?;
            return Ok(entry);
        }

        transaction.execute(
            "INSERT INTO audit_validations(
                 round_id, message_id, source_sequence, recorded_at_ms, payload_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                validation.round_id,
                validation.message_id,
                to_i64(validation.source_sequence)?,
                to_i64(recorded_at_ms)?,
                payload_json
            ],
        )?;
        let sequence = transaction.last_insert_rowid();
        apply_retention(&transaction, &self.retention, recorded_at_ms)?;
        let entry = load_entry(&transaction, sequence)?.ok_or_else(missing_database_row)?;
        transaction.commit()?;
        Ok(entry)
    }

    pub fn record_resolution(
        &mut self,
        resolution: &OperatorResolution,
    ) -> Result<AuditEntry, AuditError> {
        validate_resolution(resolution)?;
        let payload = StoredResolution {
            original_decision: resolution.original_decision.clone(),
            final_decision: resolution.final_decision.clone(),
            target_id: resolution.target_id.clone(),
            note: resolution.note.clone(),
        };
        let payload_json = serde_json::to_string(&payload).map_err(database_serialization)?;
        let recorded_at_ms = now_ms()?;
        let transaction = self.connection.transaction()?;

        let validation_row = transaction
            .query_row(
                "SELECT sequence, payload_json
                 FROM audit_validations
                 WHERE round_id = ?1 AND message_id = ?2",
                params![resolution.round_id, resolution.message_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .ok_or(AuditError::MissingValidation)?;
        let validation: StoredValidation =
            serde_json::from_str(&validation_row.1).map_err(database_serialization)?;
        if validation.participant_id != resolution.participant_id
            || validation.source_sequence != resolution.source_sequence
            || validation.decision != resolution.original_decision
        {
            return Err(AuditError::Conflict);
        }

        let existing_json = transaction
            .query_row(
                "SELECT payload_json FROM audit_resolutions WHERE validation_sequence = ?1",
                [validation_row.0],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(existing_json) = existing_json {
            if existing_json != payload_json {
                return Err(AuditError::Conflict);
            }
        } else {
            transaction.execute(
                "INSERT INTO audit_resolutions(validation_sequence, recorded_at_ms, payload_json)
                 VALUES (?1, ?2, ?3)",
                params![validation_row.0, to_i64(recorded_at_ms)?, payload_json],
            )?;
        }

        let entry = load_entry(&transaction, validation_row.0)?.ok_or_else(missing_database_row)?;
        transaction.commit()?;
        Ok(entry)
    }

    pub fn list_round(&self, round_id: &str) -> Result<Vec<AuditEntry>, AuditError> {
        if !valid_identifier(round_id) {
            return Err(AuditError::InvalidRecord);
        }
        load_entries(
            &self.connection,
            "SELECT validation.sequence, validation.recorded_at_ms, validation.payload_json,
                    resolution.recorded_at_ms, resolution.payload_json
             FROM audit_validations AS validation
             LEFT JOIN audit_resolutions AS resolution
               ON resolution.validation_sequence = validation.sequence
             WHERE validation.round_id = ?1
             ORDER BY validation.source_sequence ASC, validation.sequence ASC",
            [round_id],
        )
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<AuditEntry>, AuditError> {
        if limit > MAX_AUDIT_PAGE {
            return Err(AuditError::InvalidRecord);
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT validation.sequence, validation.recorded_at_ms, validation.payload_json,
                    resolution.recorded_at_ms, resolution.payload_json
             FROM audit_validations AS validation
             LEFT JOIN audit_resolutions AS resolution
               ON resolution.validation_sequence = validation.sequence
             ORDER BY validation.sequence DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map([to_i64(limit as u64)?], read_stored_row)?;
        decode_rows(rows)
    }

    pub fn delete_round(&mut self, round_id: &str) -> Result<usize, AuditError> {
        if !valid_identifier(round_id) {
            return Err(AuditError::InvalidRecord);
        }
        let deleted = self
            .connection
            .execute("DELETE FROM audit_validations WHERE round_id = ?1", [round_id])?;
        if deleted > 0 {
            self.connection.execute_batch("VACUUM;")?;
        }
        Ok(deleted)
    }

    pub fn purge_all(&mut self) -> Result<usize, AuditError> {
        let deleted = self.connection.execute("DELETE FROM audit_validations", [])?;
        if deleted > 0 {
            self.connection.execute_batch("VACUUM;")?;
        }
        Ok(deleted)
    }
}

fn validate_retention(retention: &RetentionPolicy) -> Result<(), AuditError> {
    if retention.max_validations == 0
        || retention.max_validations > MAX_RETAINED_VALIDATIONS
        || retention.max_age_seconds.is_some_and(|age| age == 0 || age > MAX_RETENTION_AGE_SECONDS)
    {
        return Err(AuditError::InvalidRetention);
    }
    Ok(())
}

fn validate_validation(
    validation: &Validation,
    context_package_sha256: Option<&str>,
) -> Result<(), AuditError> {
    let target_is_valid = match validation.decision {
        Decision::Accepted => {
            validation.target_id.as_ref().is_some_and(|value| valid_identifier(value))
        }
        Decision::Rejected => validation.target_id.is_none(),
        Decision::Abstained => {
            validation.target_id.as_ref().is_none_or(|value| valid_identifier(value))
        }
    };
    if !valid_identifier(&validation.round_id)
        || !valid_identifier(&validation.message_id)
        || !valid_identifier(&validation.participant_id)
        || validation.source_sequence > i64::MAX as u64
        || !validation.score.is_finite()
        || !(0.0..=1.0).contains(&validation.score)
        || validation.evidence.len() > MAX_TARGETS_PER_ROUND
        || !target_is_valid
        || validation.issue.is_some()
            && (validation.decision != Decision::Rejected || !validation.evidence.is_empty())
        || context_package_sha256.is_some_and(|value| {
            value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(AuditError::InvalidRecord);
    }
    Ok(())
}

fn validate_resolution(resolution: &OperatorResolution) -> Result<(), AuditError> {
    let target_is_valid = match resolution.final_decision {
        Decision::Accepted => {
            resolution.target_id.as_ref().is_some_and(|value| valid_identifier(value))
        }
        Decision::Rejected => resolution.target_id.is_none(),
        Decision::Abstained => false,
    };
    if !valid_identifier(&resolution.round_id)
        || !valid_identifier(&resolution.message_id)
        || !valid_identifier(&resolution.participant_id)
        || resolution.source_sequence > i64::MAX as u64
        || resolution.note.chars().count() > MAX_RESOLUTION_NOTE_CHARS
        || !target_is_valid
    {
        return Err(AuditError::InvalidRecord);
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_IDENTIFIER_CHARS
        && !value.chars().any(char::is_control)
}

fn apply_retention(
    connection: &Connection,
    retention: &RetentionPolicy,
    current_time_ms: u64,
) -> Result<(), AuditError> {
    if let Some(max_age_seconds) = retention.max_age_seconds {
        let max_age_ms = max_age_seconds.checked_mul(1_000).ok_or(AuditError::InvalidRetention)?;
        let cutoff = current_time_ms.saturating_sub(max_age_ms);
        connection.execute(
            "DELETE FROM audit_validations WHERE recorded_at_ms < ?1",
            [to_i64(cutoff)?],
        )?;
    }
    connection.execute(
        "DELETE FROM audit_validations
         WHERE sequence NOT IN (
             SELECT sequence FROM audit_validations ORDER BY sequence DESC LIMIT ?1
         )",
        [to_i64(retention.max_validations as u64)?],
    )?;
    Ok(())
}

fn load_entry(connection: &Connection, sequence: i64) -> Result<Option<AuditEntry>, AuditError> {
    let row = connection
        .query_row(
            "SELECT validation.sequence, validation.recorded_at_ms, validation.payload_json,
                    resolution.recorded_at_ms, resolution.payload_json
             FROM audit_validations AS validation
             LEFT JOIN audit_resolutions AS resolution
               ON resolution.validation_sequence = validation.sequence
             WHERE validation.sequence = ?1",
            [sequence],
            read_stored_row,
        )
        .optional()?;
    row.map(decode_row).transpose()
}

fn load_entries<P>(
    connection: &Connection,
    query: &str,
    parameters: P,
) -> Result<Vec<AuditEntry>, AuditError>
where
    P: rusqlite::Params,
{
    let mut statement = connection.prepare(query)?;
    let rows = statement.query_map(parameters, read_stored_row)?;
    decode_rows(rows)
}

fn read_stored_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRow> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
}

fn decode_rows(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<StoredRow>>,
) -> Result<Vec<AuditEntry>, AuditError> {
    rows.map(|row| row.map_err(AuditError::from).and_then(decode_row)).collect()
}

fn decode_row(row: StoredRow) -> Result<AuditEntry, AuditError> {
    let validation: StoredValidation =
        serde_json::from_str(&row.2).map_err(database_serialization)?;
    let resolution = match (row.3, row.4) {
        (Some(recorded_at_ms), Some(payload_json)) => {
            let stored: StoredResolution =
                serde_json::from_str(&payload_json).map_err(database_serialization)?;
            Some(AuditResolution {
                recorded_at_ms: to_u64(recorded_at_ms)?,
                original_decision: stored.original_decision,
                final_decision: stored.final_decision,
                target_id: stored.target_id,
                note: stored.note,
            })
        }
        (None, None) => None,
        _ => return Err(missing_database_row()),
    };
    Ok(AuditEntry {
        schema_version: AUDIT_SCHEMA_VERSION,
        validation: AuditValidation {
            sequence: to_u64(row.0)?,
            recorded_at_ms: to_u64(row.1)?,
            round_id: validation.round_id,
            message_id: validation.message_id,
            participant_id: validation.participant_id,
            source_sequence: validation.source_sequence,
            context_package_sha256: validation.context_package_sha256,
            decision: validation.decision,
            target_id: validation.target_id,
            score: validation.score,
            evidence_kinds: validation.evidence_kinds,
            issue: validation.issue,
        },
        resolution,
    })
}

fn now_ms() -> Result<u64, AuditError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AuditError::Database(error.to_string()))?;
    u64::try_from(duration.as_millis()).map_err(|_| AuditError::InvalidRecord)
}

fn to_i64(value: u64) -> Result<i64, AuditError> {
    i64::try_from(value).map_err(|_| AuditError::InvalidRecord)
}

fn to_u64(value: i64) -> Result<u64, AuditError> {
    u64::try_from(value).map_err(|_| missing_database_row())
}

fn database_serialization(error: impl fmt::Display) -> AuditError {
    AuditError::Database(error.to_string())
}

fn missing_database_row() -> AuditError {
    AuditError::Database("audit database contains an inconsistent row".to_owned())
}
