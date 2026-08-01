use std::{fmt, path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

const MAX_SESSIONS: usize = 10_000;
const MAX_EVENTS_PER_SESSION: usize = 100_000;
const MAX_DELIVERIES_PER_SESSION: usize = 1_000_000;
const MAX_DEFINITION_BYTES: usize = 8 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSessionHeader {
    pub session_id: String,
    pub definition_fingerprint: [u8; 32],
    pub definition_json: String,
    pub state: StoredSessionState,
    pub created_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub latest_event_sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredSessionState {
    Active,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEvent {
    pub sequence: u64,
    pub occurred_at_ms: u64,
    pub payload_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDelivery {
    pub message_id: String,
    pub sequence: u64,
    pub request_fingerprint: [u8; 32],
    pub validation_json: String,
    pub resolution_emitted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSession {
    pub header: StoredSessionHeader,
    pub events: Vec<StoredEvent>,
    pub deliveries: Vec<StoredDelivery>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStoreError {
    Database(String),
    InvalidRecord,
    Conflict,
    Missing,
    Ended,
}

impl fmt::Display for SessionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(message) => write!(formatter, "session database error: {message}"),
            Self::InvalidRecord => write!(formatter, "session record is invalid"),
            Self::Conflict => write!(formatter, "session record conflicts with durable state"),
            Self::Missing => write!(formatter, "durable session does not exist"),
            Self::Ended => write!(formatter, "durable session has ended"),
        }
    }
}

impl std::error::Error for SessionStoreError {}

impl From<rusqlite::Error> for SessionStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string())
    }
}

pub struct SessionStore {
    connection: Connection,
}

impl SessionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SessionStoreError> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, SessionStoreError> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(connection: Connection) -> Result<Self, SessionStoreError> {
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA secure_delete = ON;
             CREATE TABLE IF NOT EXISTS session_records (
                 session_id TEXT PRIMARY KEY,
                 definition_fingerprint BLOB NOT NULL,
                 definition_json TEXT NOT NULL,
                 state TEXT NOT NULL CHECK(state IN ('active', 'ended')),
                 created_at_ms INTEGER NOT NULL,
                 ended_at_ms INTEGER,
                 latest_event_sequence INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS session_events (
                 session_id TEXT NOT NULL REFERENCES session_records(session_id) ON DELETE CASCADE,
                 sequence INTEGER NOT NULL,
                 occurred_at_ms INTEGER NOT NULL,
                 payload_json TEXT NOT NULL,
                 PRIMARY KEY(session_id, sequence)
             );
             CREATE TABLE IF NOT EXISTS session_deliveries (
                 session_id TEXT NOT NULL REFERENCES session_records(session_id) ON DELETE CASCADE,
                 message_id TEXT NOT NULL,
                 sequence INTEGER NOT NULL,
                 request_fingerprint BLOB NOT NULL,
                 validation_json TEXT NOT NULL,
                 resolution_emitted INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY(session_id, message_id)
             );
             CREATE INDEX IF NOT EXISTS session_deliveries_order
                 ON session_deliveries(session_id, sequence);",
        )?;
        Ok(Self { connection })
    }

    pub fn load_sessions(&self) -> Result<Vec<StoredSession>, SessionStoreError> {
        let mut statement = self.connection.prepare(
            "SELECT session_id, definition_fingerprint, definition_json, state,
                    created_at_ms, ended_at_ms, latest_event_sequence
             FROM session_records ORDER BY created_at_ms ASC LIMIT ?1",
        )?;
        let headers = statement
            .query_map([to_i64(MAX_SESSIONS as u64)?], read_header)?
            .collect::<Result<Vec<_>, _>>()?;
        headers
            .into_iter()
            .map(|header| {
                let events = load_events(&self.connection, &header.session_id)?;
                let deliveries = load_deliveries(&self.connection, &header.session_id)?;
                Ok(StoredSession { header, events, deliveries })
            })
            .collect()
    }

    pub fn create_session(
        &mut self,
        header: &StoredSessionHeader,
        started: &StoredEvent,
    ) -> Result<(), SessionStoreError> {
        validate_header(header)?;
        validate_event(started)?;
        if header.state != StoredSessionState::Active
            || header.ended_at_ms.is_some()
            || header.latest_event_sequence != started.sequence
            || started.sequence != 1
        {
            return Err(SessionStoreError::InvalidRecord);
        }
        let transaction = self.connection.transaction()?;
        transaction
            .execute(
                "INSERT INTO session_records(
                session_id, definition_fingerprint, definition_json, state,
                created_at_ms, ended_at_ms, latest_event_sequence
             ) VALUES (?1, ?2, ?3, 'active', ?4, NULL, ?5)",
                params![
                    header.session_id,
                    header.definition_fingerprint.as_slice(),
                    header.definition_json,
                    to_i64(header.created_at_ms)?,
                    to_i64(header.latest_event_sequence)?
                ],
            )
            .map_err(map_insert_error)?;
        insert_event(&transaction, &header.session_id, started)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_validation(
        &mut self,
        session_id: &str,
        event: &StoredEvent,
        delivery: &StoredDelivery,
        max_events: usize,
        max_deliveries: usize,
    ) -> Result<(), SessionStoreError> {
        validate_limits(max_events, max_deliveries)?;
        validate_event(event)?;
        validate_delivery(delivery)?;
        if event.sequence != delivery.sequence {
            return Err(SessionStoreError::InvalidRecord);
        }
        let transaction = self.connection.transaction()?;
        expect_next_active_sequence(&transaction, session_id, event.sequence)?;
        transaction
            .execute(
                "INSERT INTO session_deliveries(
                    session_id, message_id, sequence, request_fingerprint, validation_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    session_id,
                    delivery.message_id,
                    to_i64(delivery.sequence)?,
                    delivery.request_fingerprint.as_slice(),
                    delivery.validation_json
                ],
            )
            .map_err(map_insert_error)?;
        append_event_and_advance(&transaction, session_id, event)?;
        trim_events(&transaction, session_id, max_events)?;
        trim_deliveries(&transaction, session_id, max_deliveries)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_resolution(
        &mut self,
        session_id: &str,
        message_id: &str,
        event: &StoredEvent,
        max_events: usize,
    ) -> Result<(), SessionStoreError> {
        validate_limits(max_events, 1)?;
        validate_event(event)?;
        let transaction = self.connection.transaction()?;
        expect_next_active_sequence(&transaction, session_id, event.sequence)?;
        let changed = transaction.execute(
            "UPDATE session_deliveries SET resolution_emitted = 1
             WHERE session_id = ?1 AND message_id = ?2 AND resolution_emitted = 0",
            params![session_id, message_id],
        )?;
        if changed != 1 {
            return Err(SessionStoreError::Conflict);
        }
        append_event_and_advance(&transaction, session_id, event)?;
        trim_events(&transaction, session_id, max_events)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn end_session(
        &mut self,
        session_id: &str,
        event: &StoredEvent,
        max_events: usize,
    ) -> Result<(), SessionStoreError> {
        validate_limits(max_events, 1)?;
        validate_event(event)?;
        let transaction = self.connection.transaction()?;
        expect_next_active_sequence(&transaction, session_id, event.sequence)?;
        insert_event(&transaction, session_id, event)?;
        let changed = transaction.execute(
            "UPDATE session_records
             SET state = 'ended', ended_at_ms = ?2, latest_event_sequence = ?3
             WHERE session_id = ?1 AND state = 'active'",
            params![session_id, to_i64(event.occurred_at_ms)?, to_i64(event.sequence)?],
        )?;
        if changed != 1 {
            return Err(SessionStoreError::Conflict);
        }
        trim_events(&transaction, session_id, max_events)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn purge_all(&mut self) -> Result<usize, SessionStoreError> {
        let deleted = self.connection.execute("DELETE FROM session_records", [])?;
        if deleted > 0 {
            self.connection.execute_batch("VACUUM;")?;
        }
        Ok(deleted)
    }

    pub fn delete_ended(&mut self, session_id: &str) -> Result<(), SessionStoreError> {
        let changed = self.connection.execute(
            "DELETE FROM session_records WHERE session_id = ?1 AND state = 'ended'",
            [session_id],
        )?;
        if changed != 1 {
            return Err(SessionStoreError::Conflict);
        }
        Ok(())
    }
}

fn read_header(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredSessionHeader> {
    let fingerprint: Vec<u8> = row.get(1)?;
    let fingerprint: [u8; 32] = fingerprint.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            32,
            rusqlite::types::Type::Blob,
            "invalid fingerprint length".into(),
        )
    })?;
    let state = match row.get::<_, String>(3)?.as_str() {
        "active" => StoredSessionState::Active,
        "ended" => StoredSessionState::Ended,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(StoredSessionHeader {
        session_id: row.get(0)?,
        definition_fingerprint: fingerprint,
        definition_json: row.get(2)?,
        state,
        created_at_ms: from_i64(row.get(4)?)?,
        ended_at_ms: row.get::<_, Option<i64>>(5)?.map(from_i64).transpose()?,
        latest_event_sequence: from_i64(row.get(6)?)?,
    })
}

fn load_events(
    connection: &Connection,
    session_id: &str,
) -> Result<Vec<StoredEvent>, SessionStoreError> {
    let mut statement = connection.prepare(
        "SELECT sequence, occurred_at_ms, payload_json FROM session_events
         WHERE session_id = ?1 ORDER BY sequence ASC",
    )?;
    Ok(statement
        .query_map([session_id], |row| {
            Ok(StoredEvent {
                sequence: from_i64(row.get(0)?)?,
                occurred_at_ms: from_i64(row.get(1)?)?,
                payload_json: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn load_deliveries(
    connection: &Connection,
    session_id: &str,
) -> Result<Vec<StoredDelivery>, SessionStoreError> {
    let mut statement = connection.prepare(
        "SELECT message_id, sequence, request_fingerprint, validation_json, resolution_emitted
         FROM session_deliveries WHERE session_id = ?1 ORDER BY sequence ASC",
    )?;
    Ok(statement
        .query_map([session_id], |row| {
            let fingerprint: Vec<u8> = row.get(2)?;
            Ok(StoredDelivery {
                message_id: row.get(0)?,
                sequence: from_i64(row.get(1)?)?,
                request_fingerprint: fingerprint.try_into().map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        32,
                        rusqlite::types::Type::Blob,
                        "invalid fingerprint length".into(),
                    )
                })?,
                validation_json: row.get(3)?,
                resolution_emitted: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn expect_next_active_sequence(
    transaction: &Transaction<'_>,
    session_id: &str,
    sequence: u64,
) -> Result<(), SessionStoreError> {
    let row = transaction
        .query_row(
            "SELECT state, latest_event_sequence FROM session_records WHERE session_id = ?1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .ok_or(SessionStoreError::Missing)?;
    if row.0 == "ended" {
        return Err(SessionStoreError::Ended);
    }
    if row.0 != "active" || sequence != from_i64(row.1)?.saturating_add(1) {
        return Err(SessionStoreError::Conflict);
    }
    Ok(())
}

fn append_event_and_advance(
    transaction: &Transaction<'_>,
    session_id: &str,
    event: &StoredEvent,
) -> Result<(), SessionStoreError> {
    insert_event(transaction, session_id, event)?;
    transaction.execute(
        "UPDATE session_records SET latest_event_sequence = ?2 WHERE session_id = ?1",
        params![session_id, to_i64(event.sequence)?],
    )?;
    Ok(())
}

fn insert_event(
    transaction: &Transaction<'_>,
    session_id: &str,
    event: &StoredEvent,
) -> Result<(), SessionStoreError> {
    transaction.execute(
        "INSERT INTO session_events(session_id, sequence, occurred_at_ms, payload_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            session_id,
            to_i64(event.sequence)?,
            to_i64(event.occurred_at_ms)?,
            event.payload_json
        ],
    )?;
    Ok(())
}

fn trim_events(
    transaction: &Transaction<'_>,
    session_id: &str,
    capacity: usize,
) -> Result<(), SessionStoreError> {
    transaction.execute(
        "DELETE FROM session_events WHERE session_id = ?1 AND sequence <= (
             SELECT MAX(sequence) - ?2 FROM session_events WHERE session_id = ?1
         )",
        params![session_id, to_i64(capacity as u64)?],
    )?;
    Ok(())
}

fn trim_deliveries(
    transaction: &Transaction<'_>,
    session_id: &str,
    capacity: usize,
) -> Result<(), SessionStoreError> {
    transaction.execute(
        "DELETE FROM session_deliveries WHERE session_id = ?1 AND sequence <= (
             SELECT MAX(sequence) - ?2 FROM session_deliveries WHERE session_id = ?1
         )",
        params![session_id, to_i64(capacity as u64)?],
    )?;
    Ok(())
}

fn validate_header(header: &StoredSessionHeader) -> Result<(), SessionStoreError> {
    if header.session_id.is_empty()
        || header.definition_json.is_empty()
        || header.definition_json.len() > MAX_DEFINITION_BYTES
        || header.ended_at_ms.is_some() != (header.state == StoredSessionState::Ended)
    {
        return Err(SessionStoreError::InvalidRecord);
    }
    Ok(())
}

fn validate_event(event: &StoredEvent) -> Result<(), SessionStoreError> {
    if event.sequence == 0
        || event.payload_json.is_empty()
        || event.payload_json.len() > MAX_PAYLOAD_BYTES
    {
        return Err(SessionStoreError::InvalidRecord);
    }
    Ok(())
}

fn validate_delivery(delivery: &StoredDelivery) -> Result<(), SessionStoreError> {
    if delivery.message_id.is_empty()
        || delivery.sequence == 0
        || delivery.validation_json.is_empty()
        || delivery.validation_json.len() > MAX_PAYLOAD_BYTES
    {
        return Err(SessionStoreError::InvalidRecord);
    }
    Ok(())
}

fn validate_limits(events: usize, deliveries: usize) -> Result<(), SessionStoreError> {
    if events == 0
        || events > MAX_EVENTS_PER_SESSION
        || deliveries == 0
        || deliveries > MAX_DELIVERIES_PER_SESSION
    {
        return Err(SessionStoreError::InvalidRecord);
    }
    Ok(())
}

fn map_insert_error(error: rusqlite::Error) -> SessionStoreError {
    match error {
        rusqlite::Error::SqliteFailure(ref failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            SessionStoreError::Conflict
        }
        other => other.into(),
    }
}

fn to_i64(value: u64) -> Result<i64, SessionStoreError> {
    i64::try_from(value).map_err(|_| SessionStoreError::InvalidRecord)
}

fn from_i64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}
