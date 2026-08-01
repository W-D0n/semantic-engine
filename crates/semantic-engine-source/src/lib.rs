use std::{
    collections::BTreeMap,
    fmt,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use semantic_engine_core::Submission;
use serde::{Deserialize, Serialize};

pub const SOURCE_CONTRACT_VERSION: u32 = 1;
pub const MAX_SOURCES: usize = 64;
pub const MAX_SETTINGS: usize = 32;

const MAX_ID_CHARS: usize = 128;
const MAX_ADAPTER_CHARS: usize = 64;
const MAX_DISPLAY_NAME_CHARS: usize = 80;
const MAX_SETTING_KEY_CHARS: usize = 64;
const MAX_SETTING_VALUE_CHARS: usize = 512;
const MAX_SETTINGS_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDesiredState {
    Paused,
    Active,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceRuntimeState {
    Paused,
    AuthenticationRequired,
    Connecting,
    Connected,
    Backoff,
    Faulted,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SourceDefinition {
    pub contract_version: u32,
    pub source_id: String,
    pub adapter: String,
    pub display_name: String,
    pub settings: BTreeMap<String, String>,
    pub credential_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SourceRecord {
    #[serde(flatten)]
    pub definition: SourceDefinition,
    pub desired_state: SourceDesiredState,
    pub revision: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CreateSource {
    pub source_id: String,
    pub adapter: String,
    pub display_name: String,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    pub credential_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct UpdateSource {
    pub expected_revision: u64,
    pub display_name: String,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    pub credential_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SourceMessage {
    pub source_id: String,
    pub message_id: String,
    pub participant_id: String,
    pub source_sequence: u64,
    pub text: String,
    pub occurred_at_ms: u64,
}

impl SourceMessage {
    #[must_use]
    pub fn into_submission(self) -> Submission {
        Submission {
            message_id: self.message_id,
            participant_id: self.participant_id,
            source_sequence: self.source_sequence,
            text: self.text,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SourceAdapterEvent {
    StateChanged { source_id: String, state: SourceRuntimeState, detail: Option<String> },
    Message(SourceMessage),
    Fault { source_id: String, code: String, retryable: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    Database(String),
    Invalid(&'static str),
    CapacityExceeded,
    Conflict,
    Missing,
    MustBePaused,
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(message) => write!(formatter, "source database error: {message}"),
            Self::Invalid(reason) => write!(formatter, "invalid source: {reason}"),
            Self::CapacityExceeded => write!(formatter, "source capacity is exhausted"),
            Self::Conflict => write!(formatter, "source revision conflicts with durable state"),
            Self::Missing => write!(formatter, "source does not exist"),
            Self::MustBePaused => write!(formatter, "source must be paused before removal"),
        }
    }
}

impl std::error::Error for SourceError {}

impl From<rusqlite::Error> for SourceError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

pub struct SourceStore {
    connection: Connection,
}

impl SourceStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SourceError> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, SourceError> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(connection: Connection) -> Result<Self, SourceError> {
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA secure_delete = ON;
             CREATE TABLE IF NOT EXISTS input_sources (
                 source_id TEXT PRIMARY KEY,
                 adapter TEXT NOT NULL,
                 display_name TEXT NOT NULL,
                 settings_json TEXT NOT NULL,
                 credential_id TEXT,
                 desired_state TEXT NOT NULL CHECK(desired_state IN ('paused', 'active')),
                 revision INTEGER NOT NULL,
                 created_at_ms INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS input_sources_adapter
                 ON input_sources(adapter, source_id);",
        )?;
        Ok(Self { connection })
    }

    pub fn add(&mut self, request: CreateSource) -> Result<SourceRecord, SourceError> {
        validate_create(&request)?;
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM input_sources", [], |row| row.get(0))?;
        if count >= i64::try_from(MAX_SOURCES).map_err(|_| SourceError::CapacityExceeded)? {
            return Err(SourceError::CapacityExceeded);
        }
        let now = now_ms()?;
        let settings_json = serde_json::to_string(&request.settings)
            .map_err(|_| SourceError::Invalid("settings cannot be serialized"))?;
        self.connection
            .execute(
                "INSERT INTO input_sources(
                    source_id, adapter, display_name, settings_json, credential_id,
                    desired_state, revision, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'paused', 1, ?6, ?6)",
                params![
                    request.source_id,
                    request.adapter,
                    request.display_name,
                    settings_json,
                    request.credential_id,
                    to_i64(now)?
                ],
            )
            .map_err(map_insert_error)?;
        self.get(&request.source_id)
    }

    pub fn list(&self) -> Result<Vec<SourceRecord>, SourceError> {
        let mut statement = self.connection.prepare(
            "SELECT source_id, adapter, display_name, settings_json, credential_id,
                    desired_state, revision, created_at_ms, updated_at_ms
             FROM input_sources ORDER BY created_at_ms, source_id LIMIT ?1",
        )?;
        statement
            .query_map([to_i64(MAX_SOURCES as u64)?], read_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(SourceError::from)
    }

    pub fn get(&self, source_id: &str) -> Result<SourceRecord, SourceError> {
        validate_identifier(source_id, MAX_ID_CHARS, "source identifier is invalid")?;
        self.connection
            .query_row(
                "SELECT source_id, adapter, display_name, settings_json, credential_id,
                        desired_state, revision, created_at_ms, updated_at_ms
                 FROM input_sources WHERE source_id = ?1",
                [source_id],
                read_record,
            )
            .optional()?
            .ok_or(SourceError::Missing)
    }

    pub fn update(
        &mut self,
        source_id: &str,
        request: UpdateSource,
    ) -> Result<SourceRecord, SourceError> {
        validate_identifier(source_id, MAX_ID_CHARS, "source identifier is invalid")?;
        validate_display_name(&request.display_name)?;
        validate_settings(&request.settings)?;
        validate_credential_id(request.credential_id.as_deref())?;
        if request.expected_revision == 0 {
            return Err(SourceError::Invalid("expected revision must be positive"));
        }
        let settings_json = serde_json::to_string(&request.settings)
            .map_err(|_| SourceError::Invalid("settings cannot be serialized"))?;
        let changed = self.connection.execute(
            "UPDATE input_sources
             SET display_name = ?1, settings_json = ?2, credential_id = ?3,
                 revision = revision + 1, updated_at_ms = ?4
             WHERE source_id = ?5 AND revision = ?6",
            params![
                request.display_name,
                settings_json,
                request.credential_id,
                to_i64(now_ms()?)?,
                source_id,
                to_i64(request.expected_revision)?
            ],
        )?;
        self.expect_changed(source_id, changed)?;
        self.get(source_id)
    }

    pub fn set_desired_state(
        &mut self,
        source_id: &str,
        expected_revision: u64,
        state: SourceDesiredState,
    ) -> Result<SourceRecord, SourceError> {
        validate_identifier(source_id, MAX_ID_CHARS, "source identifier is invalid")?;
        if expected_revision == 0 {
            return Err(SourceError::Invalid("expected revision must be positive"));
        }
        let changed = self.connection.execute(
            "UPDATE input_sources
             SET desired_state = ?1, revision = revision + 1, updated_at_ms = ?2
             WHERE source_id = ?3 AND revision = ?4 AND desired_state != ?1",
            params![state.as_str(), to_i64(now_ms()?)?, source_id, to_i64(expected_revision)?],
        )?;
        if changed == 0 {
            let current = self.get(source_id)?;
            if current.revision != expected_revision {
                return Err(SourceError::Conflict);
            }
            return Ok(current);
        }
        self.get(source_id)
    }

    pub fn remove(&mut self, source_id: &str, expected_revision: u64) -> Result<(), SourceError> {
        let current = self.get(source_id)?;
        if current.revision != expected_revision {
            return Err(SourceError::Conflict);
        }
        if current.desired_state != SourceDesiredState::Paused {
            return Err(SourceError::MustBePaused);
        }
        let changed = self.connection.execute(
            "DELETE FROM input_sources WHERE source_id = ?1 AND revision = ?2",
            params![source_id, to_i64(expected_revision)?],
        )?;
        self.expect_changed(source_id, changed).map(|_| ())
    }

    fn expect_changed(&self, source_id: &str, changed: usize) -> Result<(), SourceError> {
        if changed == 1 {
            return Ok(());
        }
        let exists = self
            .connection
            .query_row("SELECT 1 FROM input_sources WHERE source_id = ?1", [source_id], |_| Ok(()))
            .optional()?
            .is_some();
        Err(if exists { SourceError::Conflict } else { SourceError::Missing })
    }
}

impl SourceDesiredState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Paused => "paused",
            Self::Active => "active",
        }
    }
}

fn validate_create(request: &CreateSource) -> Result<(), SourceError> {
    validate_identifier(&request.source_id, MAX_ID_CHARS, "source identifier is invalid")?;
    validate_identifier(&request.adapter, MAX_ADAPTER_CHARS, "adapter identifier is invalid")?;
    validate_display_name(&request.display_name)?;
    validate_settings(&request.settings)?;
    validate_credential_id(request.credential_id.as_deref())
}

fn validate_display_name(value: &str) -> Result<(), SourceError> {
    let trimmed = value.trim();
    if trimmed != value
        || trimmed.is_empty()
        || value.chars().count() > MAX_DISPLAY_NAME_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(SourceError::Invalid("display name is invalid"));
    }
    Ok(())
}

fn validate_settings(settings: &BTreeMap<String, String>) -> Result<(), SourceError> {
    if settings.len() > MAX_SETTINGS {
        return Err(SourceError::Invalid("too many source settings"));
    }
    let mut total_bytes = 0usize;
    for (key, value) in settings {
        validate_identifier(key, MAX_SETTING_KEY_CHARS, "setting key is invalid")?;
        if is_sensitive_key(key) {
            return Err(SourceError::Invalid(
                "secrets must use credential_id instead of source settings",
            ));
        }
        if value.chars().count() > MAX_SETTING_VALUE_CHARS || value.chars().any(char::is_control) {
            return Err(SourceError::Invalid("setting value is invalid"));
        }
        total_bytes = total_bytes.saturating_add(key.len()).saturating_add(value.len());
    }
    if total_bytes > MAX_SETTINGS_BYTES {
        return Err(SourceError::Invalid("source settings are too large"));
    }
    Ok(())
}

fn validate_credential_id(value: Option<&str>) -> Result<(), SourceError> {
    if let Some(value) = value {
        validate_identifier(value, MAX_ID_CHARS, "credential identifier is invalid")?;
    }
    Ok(())
}

fn validate_identifier(
    value: &str,
    max_chars: usize,
    reason: &'static str,
) -> Result<(), SourceError> {
    if value.is_empty()
        || value.chars().count() > max_chars
        || value.starts_with(['.', '-'])
        || value.ends_with(['.', '-'])
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(SourceError::Invalid(reason));
    }
    Ok(())
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    ["token", "secret", "password", "private_key", "authorization", "cookie"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn read_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRecord> {
    let state: String = row.get(5)?;
    let desired_state = match state.as_str() {
        "paused" => SourceDesiredState::Paused,
        "active" => SourceDesiredState::Active,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let settings_json: String = row.get(3)?;
    let settings = serde_json::from_str(&settings_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            settings_json.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(SourceRecord {
        definition: SourceDefinition {
            contract_version: SOURCE_CONTRACT_VERSION,
            source_id: row.get(0)?,
            adapter: row.get(1)?,
            display_name: row.get(2)?,
            settings,
            credential_id: row.get(4)?,
        },
        desired_state,
        revision: from_i64(row.get(6)?)?,
        created_at_ms: from_i64(row.get(7)?)?,
        updated_at_ms: from_i64(row.get(8)?)?,
    })
}

fn map_insert_error(error: rusqlite::Error) -> SourceError {
    if matches!(error.sqlite_error_code(), Some(ErrorCode::ConstraintViolation)) {
        SourceError::Conflict
    } else {
        SourceError::from(error)
    }
}

fn now_ms() -> Result<u64, SourceError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SourceError::Invalid("system clock is before the Unix epoch"))?;
    u64::try_from(duration.as_millis())
        .map_err(|_| SourceError::Invalid("system clock cannot be represented"))
}

fn to_i64(value: u64) -> Result<i64, SourceError> {
    i64::try_from(value).map_err(|_| SourceError::Invalid("numeric value is too large"))
}

fn from_i64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
