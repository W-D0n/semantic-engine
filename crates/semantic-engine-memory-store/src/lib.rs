//! Durable, bounded recognition memory populated only by explicit operator actions.

use std::{collections::HashSet, fmt, path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use semantic_engine_core::{MAX_EXPRESSION_CHARS, MAX_IDENTIFIER_CHARS, normalize_expression};
use serde::{Deserialize, Serialize};

const MAX_MEMORY_CAPACITY: usize = 100_000;
const MAX_MEMORY_TTL: Duration = Duration::from_secs(365 * 24 * 60 * 60);
const MAX_MEMORY_PAGE: usize = 1_000;
pub const NORMALIZATION_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryPolicy {
    pub capacity: usize,
    pub ttl: Duration,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self { capacity: 1_000, ttl: Duration::from_secs(30 * 24 * 60 * 60) }
    }
}

impl MemoryPolicy {
    pub fn validate(&self) -> Result<(), MemoryError> {
        validate_policy(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    Active,
    Revoked,
    Expired,
    Evicted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub context_package_sha256: String,
    pub target_id: String,
    pub expression: String,
    pub normalized_expression: String,
    pub normalization_version: u32,
    pub source_resolution_sha256: String,
    pub created_at_ms: u64,
    pub last_used_at_ms: u64,
    pub expires_at_ms: u64,
    pub use_count: u64,
    pub state: MemoryState,
    pub state_changed_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryError {
    InvalidPolicy,
    InvalidEntry,
    Missing,
    EntropyUnavailable,
    Database(String),
}

impl fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy => formatter.write_str("recognition memory policy is invalid"),
            Self::InvalidEntry => formatter.write_str("recognition memory entry is invalid"),
            Self::Missing => formatter.write_str("recognition memory entry does not exist"),
            Self::EntropyUnavailable => {
                formatter.write_str("OS randomness is unavailable for recognition memory")
            }
            Self::Database(message) => {
                write!(formatter, "recognition memory database error: {message}")
            }
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<rusqlite::Error> for MemoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

pub struct RecognitionMemoryStore {
    connection: Connection,
    policy: MemoryPolicy,
}

impl RecognitionMemoryStore {
    pub fn open(path: impl AsRef<Path>, policy: MemoryPolicy) -> Result<Self, MemoryError> {
        Self::initialize(Connection::open(path)?, policy)
    }

    pub fn open_in_memory(policy: MemoryPolicy) -> Result<Self, MemoryError> {
        Self::initialize(Connection::open_in_memory()?, policy)
    }

    fn initialize(connection: Connection, policy: MemoryPolicy) -> Result<Self, MemoryError> {
        validate_policy(&policy)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA secure_delete = ON;
             CREATE TABLE IF NOT EXISTS recognition_memory (
                 row_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 memory_id TEXT NOT NULL UNIQUE,
                 context_package_sha256 TEXT NOT NULL,
                 target_id TEXT NOT NULL,
                 expression TEXT NOT NULL,
                 normalized_expression TEXT NOT NULL,
                 normalization_version INTEGER NOT NULL,
                 source_resolution_sha256 TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL,
                 last_used_at_ms INTEGER NOT NULL,
                 expires_at_ms INTEGER NOT NULL,
                 use_count INTEGER NOT NULL DEFAULT 0,
                 state TEXT NOT NULL CHECK(state IN ('active', 'revoked', 'expired', 'evicted')),
                 state_changed_at_ms INTEGER
             );
             CREATE INDEX IF NOT EXISTS recognition_memory_lookup
                 ON recognition_memory(context_package_sha256, normalized_expression, state);
             CREATE INDEX IF NOT EXISTS recognition_memory_lru
                 ON recognition_memory(state, last_used_at_ms, row_id);",
        )?;
        Ok(Self { connection, policy })
    }

    pub fn remember(
        &mut self,
        context_package_sha256: &str,
        target_id: &str,
        expression: &str,
        source_resolution_sha256: &[u8; 32],
        now_ms: u64,
    ) -> Result<MemoryEntry, MemoryError> {
        validate_context(context_package_sha256)?;
        validate_identifier(target_id)?;
        validate_expression(expression)?;
        let normalized = normalize_expression(expression);
        if normalized.is_empty() {
            return Err(MemoryError::InvalidEntry);
        }
        let ttl_ms =
            u64::try_from(self.policy.ttl.as_millis()).map_err(|_| MemoryError::InvalidPolicy)?;
        let expires_at_ms = now_ms.checked_add(ttl_ms).ok_or(MemoryError::InvalidEntry)?;
        let context = context_package_sha256.to_ascii_lowercase();
        let source = hex(source_resolution_sha256);
        let transaction = self.connection.transaction()?;
        expire_entries(&transaction, now_ms)?;

        let existing_row_id = transaction
            .query_row(
                "SELECT row_id FROM recognition_memory
                 WHERE context_package_sha256 = ?1 AND target_id = ?2
                   AND normalized_expression = ?3 AND state = 'active'
                   AND normalization_version = ?4
                 ORDER BY row_id DESC LIMIT 1",
                params![context, target_id, normalized, NORMALIZATION_VERSION],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(existing_row_id) = existing_row_id {
            let entry =
                load_entry_by_row(&transaction, existing_row_id)?.ok_or(MemoryError::Missing)?;
            transaction.commit()?;
            return Ok(entry);
        }

        evict_if_full(&transaction, self.policy.capacity, now_ms)?;
        let memory_id = generate_memory_id()?;
        transaction.execute(
            "INSERT INTO recognition_memory(
                 memory_id, context_package_sha256, target_id, expression, normalized_expression,
                 normalization_version,
                 source_resolution_sha256, created_at_ms, last_used_at_ms, expires_at_ms,
                 use_count, state, state_changed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, 0, 'active', NULL)",
            params![
                memory_id,
                context,
                target_id,
                expression,
                normalized,
                NORMALIZATION_VERSION,
                source,
                to_i64(now_ms)?,
                to_i64(expires_at_ms)?
            ],
        )?;
        let row_id = transaction.last_insert_rowid();
        trim_history(&transaction, self.policy.capacity)?;
        let entry = load_entry_by_row(&transaction, row_id)?.ok_or(MemoryError::Missing)?;
        transaction.commit()?;
        Ok(entry)
    }

    pub fn lookup(
        &mut self,
        context_package_sha256: &str,
        expression: &str,
        allowed_target_ids: &[String],
        now_ms: u64,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        validate_context(context_package_sha256)?;
        validate_expression(expression)?;
        if allowed_target_ids.is_empty()
            || allowed_target_ids.iter().any(|target_id| validate_identifier(target_id).is_err())
        {
            return Err(MemoryError::InvalidEntry);
        }
        let normalized = normalize_expression(expression);
        if normalized.is_empty() {
            return Ok(Vec::new());
        }
        let allowed = allowed_target_ids.iter().map(String::as_str).collect::<HashSet<_>>();
        let transaction = self.connection.transaction()?;
        expire_entries(&transaction, now_ms)?;
        let mut statement = transaction.prepare(
            "SELECT memory_id, context_package_sha256, target_id, expression, normalized_expression,
                    normalization_version,
                    source_resolution_sha256, created_at_ms, last_used_at_ms, expires_at_ms,
                    use_count, state, state_changed_at_ms
             FROM recognition_memory
             WHERE context_package_sha256 = ?1 AND normalized_expression = ?2
               AND normalization_version = ?3 AND state = 'active'
             ORDER BY row_id ASC",
        )?;
        let entries = statement
            .query_map(
                params![
                    context_package_sha256.to_ascii_lowercase(),
                    normalized,
                    NORMALIZATION_VERSION
                ],
                read_entry,
            )?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|entry| allowed.contains(entry.target_id.as_str()))
            .collect::<Vec<_>>();
        drop(statement);
        transaction.commit()?;
        Ok(entries)
    }

    pub fn mark_used(
        &mut self,
        context_package_sha256: &str,
        memory_ids: &[String],
        now_ms: u64,
    ) -> Result<(), MemoryError> {
        validate_context(context_package_sha256)?;
        if memory_ids.is_empty() {
            return Ok(());
        }
        let transaction = self.connection.transaction()?;
        for memory_id in memory_ids {
            validate_memory_id(memory_id)?;
            let changed = transaction.execute(
                "UPDATE recognition_memory
                 SET last_used_at_ms = ?3, use_count = use_count + 1
                 WHERE memory_id = ?1 AND context_package_sha256 = ?2 AND state = 'active'",
                params![memory_id, context_package_sha256.to_ascii_lowercase(), to_i64(now_ms)?],
            )?;
            if changed != 1 {
                return Err(MemoryError::Missing);
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn list(
        &mut self,
        context_package_sha256: &str,
        limit: usize,
        now_ms: u64,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        validate_context(context_package_sha256)?;
        if limit > MAX_MEMORY_PAGE {
            return Err(MemoryError::InvalidEntry);
        }
        expire_entries(&self.connection, now_ms)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT memory_id, context_package_sha256, target_id, expression, normalized_expression,
                    normalization_version,
                    source_resolution_sha256, created_at_ms, last_used_at_ms, expires_at_ms,
                    use_count, state, state_changed_at_ms
             FROM recognition_memory WHERE context_package_sha256 = ?1
             ORDER BY row_id DESC LIMIT ?2",
        )?;
        Ok(statement
            .query_map(
                params![context_package_sha256.to_ascii_lowercase(), to_i64(limit as u64)?],
                read_entry,
            )?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_active(
        &mut self,
        context_package_sha256: &str,
        limit: usize,
        now_ms: u64,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        validate_context(context_package_sha256)?;
        if limit > MAX_MEMORY_PAGE {
            return Err(MemoryError::InvalidEntry);
        }
        expire_entries(&self.connection, now_ms)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT memory_id, context_package_sha256, target_id, expression, normalized_expression,
                    normalization_version, source_resolution_sha256, created_at_ms,
                    last_used_at_ms, expires_at_ms, use_count, state, state_changed_at_ms
             FROM recognition_memory
             WHERE context_package_sha256 = ?1 AND state = 'active'
             ORDER BY row_id DESC LIMIT ?2",
        )?;
        Ok(statement
            .query_map(
                params![context_package_sha256.to_ascii_lowercase(), to_i64(limit as u64)?],
                read_entry,
            )?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn revoke(
        &mut self,
        context_package_sha256: &str,
        memory_id: &str,
        now_ms: u64,
    ) -> Result<MemoryEntry, MemoryError> {
        validate_context(context_package_sha256)?;
        validate_memory_id(memory_id)?;
        let changed = self.connection.execute(
            "UPDATE recognition_memory SET state = 'revoked', state_changed_at_ms = ?3
             WHERE memory_id = ?1 AND context_package_sha256 = ?2 AND state = 'active'",
            params![memory_id, context_package_sha256.to_ascii_lowercase(), to_i64(now_ms)?],
        )?;
        if changed == 0 {
            return Err(MemoryError::Missing);
        }
        load_entry_by_id(&self.connection, memory_id)?.ok_or(MemoryError::Missing)
    }

    pub fn purge_all(&mut self) -> Result<usize, MemoryError> {
        let deleted = self.connection.execute("DELETE FROM recognition_memory", [])?;
        if deleted > 0 {
            self.connection.execute_batch("VACUUM;")?;
        }
        Ok(deleted)
    }
}

fn validate_policy(policy: &MemoryPolicy) -> Result<(), MemoryError> {
    if policy.capacity == 0
        || policy.capacity > MAX_MEMORY_CAPACITY
        || policy.ttl.is_zero()
        || policy.ttl > MAX_MEMORY_TTL
    {
        return Err(MemoryError::InvalidPolicy);
    }
    Ok(())
}

fn validate_context(value: &str) -> Result<(), MemoryError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(MemoryError::InvalidEntry);
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<(), MemoryError> {
    if value.is_empty()
        || value.chars().count() > MAX_IDENTIFIER_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(MemoryError::InvalidEntry);
    }
    Ok(())
}

fn validate_expression(value: &str) -> Result<(), MemoryError> {
    if value.is_empty()
        || value.chars().count() > MAX_EXPRESSION_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(MemoryError::InvalidEntry);
    }
    Ok(())
}

fn expire_entries(connection: &Connection, now_ms: u64) -> Result<(), MemoryError> {
    connection.execute(
        "UPDATE recognition_memory SET state = 'expired', state_changed_at_ms = ?1
         WHERE state = 'active' AND expires_at_ms <= ?1",
        [to_i64(now_ms)?],
    )?;
    Ok(())
}

fn evict_if_full(
    transaction: &Transaction<'_>,
    capacity: usize,
    now_ms: u64,
) -> Result<(), MemoryError> {
    let active = transaction.query_row(
        "SELECT COUNT(*) FROM recognition_memory WHERE state = 'active'",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if usize::try_from(active).map_err(|_| MemoryError::InvalidEntry)? >= capacity {
        transaction.execute(
            "UPDATE recognition_memory SET state = 'evicted', state_changed_at_ms = ?1
             WHERE row_id = (
                 SELECT row_id FROM recognition_memory WHERE state = 'active'
                 ORDER BY last_used_at_ms ASC, row_id ASC LIMIT 1
             )",
            [to_i64(now_ms)?],
        )?;
    }
    Ok(())
}

fn trim_history(transaction: &Transaction<'_>, capacity: usize) -> Result<(), MemoryError> {
    let history_capacity = capacity.saturating_mul(4);
    transaction.execute(
        "DELETE FROM recognition_memory WHERE state != 'active' AND row_id NOT IN (
             SELECT row_id FROM recognition_memory WHERE state != 'active'
             ORDER BY row_id DESC LIMIT ?1
         )",
        [to_i64(history_capacity as u64)?],
    )?;
    Ok(())
}

fn load_entry_by_row(
    connection: &Connection,
    row_id: i64,
) -> Result<Option<MemoryEntry>, MemoryError> {
    Ok(connection
        .query_row(
            "SELECT memory_id, context_package_sha256, target_id, expression, normalized_expression,
                    normalization_version,
                    source_resolution_sha256, created_at_ms, last_used_at_ms, expires_at_ms,
                    use_count, state, state_changed_at_ms
             FROM recognition_memory WHERE row_id = ?1",
            [row_id],
            read_entry,
        )
        .optional()?)
}

fn load_entry_by_id(
    connection: &Connection,
    memory_id: &str,
) -> Result<Option<MemoryEntry>, MemoryError> {
    Ok(connection
        .query_row(
            "SELECT memory_id, context_package_sha256, target_id, expression, normalized_expression,
                    normalization_version, source_resolution_sha256, created_at_ms,
                    last_used_at_ms, expires_at_ms, use_count, state, state_changed_at_ms
             FROM recognition_memory WHERE memory_id = ?1",
            [memory_id],
            read_entry,
        )
        .optional()?)
}

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryEntry> {
    let state = match row.get::<_, String>(11)?.as_str() {
        "active" => MemoryState::Active,
        "revoked" => MemoryState::Revoked,
        "expired" => MemoryState::Expired,
        "evicted" => MemoryState::Evicted,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(MemoryEntry {
        id: row.get(0)?,
        context_package_sha256: row.get(1)?,
        target_id: row.get(2)?,
        expression: row.get(3)?,
        normalized_expression: row.get(4)?,
        normalization_version: u32::try_from(row.get::<_, i64>(5)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        source_resolution_sha256: row.get(6)?,
        created_at_ms: from_i64(row.get(7)?)?,
        last_used_at_ms: from_i64(row.get(8)?)?,
        expires_at_ms: from_i64(row.get(9)?)?,
        use_count: from_i64(row.get(10)?)?,
        state,
        state_changed_at_ms: row.get::<_, Option<i64>>(12)?.map(from_i64).transpose()?,
    })
}

fn to_i64(value: u64) -> Result<i64, MemoryError> {
    i64::try_from(value).map_err(|_| MemoryError::InvalidEntry)
}

fn from_i64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn generate_memory_id() -> Result<String, MemoryError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| MemoryError::EntropyUnavailable)?;
    Ok(hex16(&bytes))
}

fn validate_memory_id(value: &str) -> Result<(), MemoryError> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(MemoryError::InvalidEntry);
    }
    Ok(())
}

fn hex16(bytes: &[u8; 16]) -> String {
    let mut output = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}
