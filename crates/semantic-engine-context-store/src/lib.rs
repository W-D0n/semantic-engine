use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    fmt,
    path::Path,
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use semantic_engine_core::{
    AnswerTarget, MAX_ALIASES_PER_TARGET, MAX_EXPRESSION_CHARS, MAX_IDENTIFIER_CHARS,
};
use semantic_engine_package::{
    ContextAttachment, ContextPackageDraft, ContextTargetKind, ImportedContext, LicenseMetadata,
    SourceMetadata,
};
use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const MAX_TARGET_SEARCH_RESULTS: usize = 100;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StoredContext {
    #[serde(default)]
    pub storage_format_version: u32,
    pub name: String,
    pub package_id: String,
    pub version: String,
    pub license: String,
    pub locales: Vec<String>,
    pub sources: Vec<SourceMetadata>,
    #[serde(default)]
    pub licenses: Vec<LicenseMetadata>,
    pub target_count: usize,
    pub package_sha256: String,
    pub targets_sha256: String,
    pub targets: Vec<AnswerTarget>,
    #[serde(default)]
    pub target_kinds: HashMap<String, ContextTargetKind>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub attachments: BTreeMap<String, ContextAttachment>,
    #[serde(default)]
    pub targets_resource_metadata: BTreeMap<String, serde_json::Value>,
}

impl StoredContext {
    fn from_imported(context: &ImportedContext) -> Self {
        Self {
            storage_format_version: 1,
            name: context.name.clone(),
            package_id: context.id.clone(),
            version: context.version.to_string(),
            license: context.spdx_license_expression.clone(),
            locales: context.locales.clone(),
            sources: context.sources.clone(),
            licenses: context.licenses.clone(),
            target_count: context.targets.len(),
            package_sha256: context.package_sha256.clone(),
            targets_sha256: context.targets_sha256.clone(),
            targets: context.targets.clone(),
            target_kinds: context.target_kinds.clone(),
            metadata: context.metadata.clone(),
            attachments: context.attachments.clone(),
            targets_resource_metadata: context.targets_resource_metadata.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TargetRecord {
    pub id: String,
    pub canonical: String,
    pub aliases: Vec<String>,
    pub is_draft: bool,
    pub package_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelPackageStatus {
    pub channel_root_sha256: String,
    pub archive_sha256: String,
    pub package_id: String,
    pub package_version: String,
    pub revocation_reason: Option<String>,
}

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    ImmutableVersionConflict { package_id: String, version: String },
    NoActiveContext,
    UnknownTarget(String),
    InvalidTargetDraft(&'static str),
    InvalidSearch,
    ActiveContextChanged,
    ContextMetadataUpgradeRequired,
    RevokedByTrustedChannel { package_id: String, version: String },
    InvalidChannelStatus,
    ChannelArchiveIdentityConflict,
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
            Self::NoActiveContext => write!(formatter, "no active context"),
            Self::UnknownTarget(target_id) => {
                write!(formatter, "target does not belong to the active context: {target_id}")
            }
            Self::InvalidTargetDraft(reason) => write!(formatter, "invalid target draft: {reason}"),
            Self::InvalidSearch => write!(formatter, "target search is outside supported limits"),
            Self::ActiveContextChanged => {
                write!(formatter, "active context changed; reload targets before writing")
            }
            Self::ContextMetadataUpgradeRequired => write!(
                formatter,
                "active context metadata is outdated; reactivate its original package before export"
            ),
            Self::RevokedByTrustedChannel { package_id, version } => write!(
                formatter,
                "context {package_id} version {version} was revoked by a trusted channel"
            ),
            Self::InvalidChannelStatus => write!(formatter, "invalid trusted channel status"),
            Self::ChannelArchiveIdentityConflict => {
                write!(formatter, "trusted channel archive identity changed for immutable bytes")
            }
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::ImmutableVersionConflict { .. }
            | Self::NoActiveContext
            | Self::UnknownTarget(_)
            | Self::InvalidTargetDraft(_)
            | Self::InvalidSearch
            | Self::ActiveContextChanged
            | Self::ContextMetadataUpgradeRequired
            | Self::RevokedByTrustedChannel { .. }
            | Self::InvalidChannelStatus
            | Self::ChannelArchiveIdentityConflict => None,
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
            CREATE TABLE IF NOT EXISTS target_drafts (
                package_sha256 TEXT NOT NULL REFERENCES context_versions(package_sha256),
                target_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                PRIMARY KEY(package_sha256, target_id)
            );
            CREATE TABLE IF NOT EXISTS context_channel_status (
                channel_root_sha256 TEXT NOT NULL,
                archive_sha256 TEXT NOT NULL,
                package_id TEXT NOT NULL,
                version TEXT NOT NULL,
                revocation_reason TEXT,
                PRIMARY KEY(channel_root_sha256, archive_sha256)
            );
            CREATE INDEX IF NOT EXISTS context_channel_revoked_identity
                ON context_channel_status(package_id, version)
                WHERE revocation_reason IS NOT NULL;
            INSERT OR IGNORE INTO context_state(singleton, active_sequence) VALUES (1, NULL);
            ",
        )?;
        Ok(Self { connection })
    }

    pub fn activate(&mut self, imported: &ImportedContext) -> Result<StoredContext, StoreError> {
        let context = StoredContext::from_imported(imported);
        if identity_is_revoked(&self.connection, &context.package_id, &context.version)? {
            return Err(StoreError::RevokedByTrustedChannel {
                package_id: context.package_id,
                version: context.version,
            });
        }
        let payload = serde_json::to_string(&context)?;
        let transaction =
            self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = current_pointer(&transaction)?;

        if current.as_ref().is_some_and(|(_, hash)| hash == &context.package_sha256) {
            transaction.execute(
                "UPDATE context_versions SET payload_json = ?1 WHERE package_sha256 = ?2",
                params![payload, context.package_sha256],
            )?;
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
        match stored_identity {
            Some((stored_hash, _)) if stored_hash == context.package_sha256 => {
                transaction.execute(
                    "UPDATE context_versions SET payload_json = ?1 WHERE package_sha256 = ?2",
                    params![payload, context.package_sha256],
                )?;
            }
            _ => {
                return Err(StoreError::ImmutableVersionConflict {
                    package_id: context.package_id.clone(),
                    version: context.version.clone(),
                });
            }
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
        active_context(&self.connection)
    }

    pub fn apply_channel_statuses(
        &mut self,
        statuses: &[ChannelPackageStatus],
    ) -> Result<Option<StoredContext>, StoreError> {
        let transaction =
            self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for status in statuses {
            validate_channel_status(status)?;
            let existing = transaction
                .query_row(
                    "SELECT package_id, version FROM context_channel_status
                     WHERE channel_root_sha256 = ?1 AND archive_sha256 = ?2",
                    params![status.channel_root_sha256, status.archive_sha256],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            if existing.as_ref().is_some_and(|(package_id, version)| {
                package_id != &status.package_id || version != &status.package_version
            }) {
                return Err(StoreError::ChannelArchiveIdentityConflict);
            }
            transaction.execute(
                "INSERT INTO context_channel_status
                 (channel_root_sha256, archive_sha256, package_id, version, revocation_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(channel_root_sha256, archive_sha256) DO UPDATE SET
                   revocation_reason = COALESCE(
                     context_channel_status.revocation_reason,
                     excluded.revocation_reason
                   )",
                params![
                    status.channel_root_sha256,
                    status.archive_sha256,
                    status.package_id,
                    status.package_version,
                    status.revocation_reason,
                ],
            )?;
        }

        let active = active_context(&transaction)?;
        let quarantined = if let Some(context) = active
            && identity_is_revoked(&transaction, &context.package_id, &context.version)?
        {
            transaction.execute(
                "UPDATE context_state SET active_sequence = NULL WHERE singleton = 1",
                [],
            )?;
            Some(context)
        } else {
            None
        };
        transaction.commit()?;
        Ok(quarantined)
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

    pub fn find_targets(&self, query: &str, limit: usize) -> Result<Vec<TargetRecord>, StoreError> {
        if limit == 0
            || limit > MAX_TARGET_SEARCH_RESULTS
            || query.chars().count() > MAX_EXPRESSION_CHARS
        {
            return Err(StoreError::InvalidSearch);
        }
        let context = self.current()?.ok_or(StoreError::NoActiveContext)?;
        let normalized_query = normalize_search(query);
        let drafts = self.target_drafts(&context.package_sha256)?;
        let mut records = Vec::with_capacity(limit);

        for published in &context.targets {
            let draft = drafts.get(&published.id);
            let target = draft.unwrap_or(published);
            let matches = normalized_query.is_empty()
                || std::iter::once(&target.canonical)
                    .chain(target.aliases.iter())
                    .any(|expression| normalize_search(expression).contains(&normalized_query));
            if matches {
                records.push(TargetRecord {
                    id: target.id.clone(),
                    canonical: target.canonical.clone(),
                    aliases: target.aliases.clone(),
                    is_draft: draft.is_some(),
                    package_sha256: context.package_sha256.clone(),
                });
            }
            if records.len() == limit {
                break;
            }
        }
        Ok(records)
    }

    pub fn save_target_draft(
        &mut self,
        package_sha256: &str,
        target: AnswerTarget,
    ) -> Result<TargetRecord, StoreError> {
        validate_target_draft(&target)?;
        let payload = serde_json::to_string(&target)?;
        let transaction =
            self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let context = active_context(&transaction)?.ok_or(StoreError::NoActiveContext)?;
        ensure_active_package(&context, package_sha256)?;
        if !context.targets.iter().any(|published| published.id == target.id) {
            return Err(StoreError::UnknownTarget(target.id));
        }
        transaction.execute(
            "INSERT INTO target_drafts(package_sha256, target_id, payload_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(package_sha256, target_id)
             DO UPDATE SET payload_json = excluded.payload_json",
            params![context.package_sha256, target.id, payload],
        )?;
        transaction.commit()?;
        Ok(TargetRecord {
            id: target.id,
            canonical: target.canonical,
            aliases: target.aliases,
            is_draft: true,
            package_sha256: context.package_sha256,
        })
    }

    pub fn discard_target_draft(
        &mut self,
        package_sha256: &str,
        target_id: &str,
    ) -> Result<bool, StoreError> {
        let transaction =
            self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let context = active_context(&transaction)?.ok_or(StoreError::NoActiveContext)?;
        ensure_active_package(&context, package_sha256)?;
        let removed = transaction.execute(
            "DELETE FROM target_drafts WHERE package_sha256 = ?1 AND target_id = ?2",
            params![context.package_sha256, target_id],
        )?;
        transaction.commit()?;
        Ok(removed > 0)
    }

    pub fn exportable_draft(
        &self,
        package_sha256: &str,
    ) -> Result<ContextPackageDraft, StoreError> {
        let context = self.current()?.ok_or(StoreError::NoActiveContext)?;
        ensure_active_package(&context, package_sha256)?;
        if context.storage_format_version != 1
            || context.licenses.is_empty()
            || context.target_kinds.len() != context.targets.len()
            || context.targets.iter().any(|target| !context.target_kinds.contains_key(&target.id))
        {
            return Err(StoreError::ContextMetadataUpgradeRequired);
        }
        let drafts = self.target_drafts(&context.package_sha256)?;
        let targets = context
            .targets
            .iter()
            .map(|published| drafts.get(&published.id).unwrap_or(published).clone())
            .collect::<Vec<_>>();
        Ok(ContextPackageDraft {
            name: context.name,
            id: context.package_id,
            base_version: context.version,
            spdx_license_expression: context.license,
            licenses: context.licenses,
            locales: context.locales,
            sources: context.sources,
            targets,
            target_kinds: context.target_kinds,
            metadata: context.metadata,
            attachments: context.attachments,
            targets_resource_metadata: context.targets_resource_metadata,
        })
    }

    fn target_drafts(
        &self,
        package_sha256: &str,
    ) -> Result<HashMap<String, AnswerTarget>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT target_id, payload_json FROM target_drafts WHERE package_sha256 = ?1",
        )?;
        let rows = statement.query_map([package_sha256], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut drafts = HashMap::new();
        for row in rows {
            let (target_id, payload) = row?;
            let target = serde_json::from_str(&payload)?;
            drafts.insert(target_id, target);
        }
        Ok(drafts)
    }
}

fn active_context(connection: &Connection) -> Result<Option<StoredContext>, StoreError> {
    let payload = connection
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

fn identity_is_revoked(
    connection: &Connection,
    package_id: &str,
    version: &str,
) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM context_channel_status
           WHERE package_id = ?1 AND version = ?2 AND revocation_reason IS NOT NULL
         )",
        params![package_id, version],
        |row| row.get(0),
    )
}

fn validate_channel_status(status: &ChannelPackageStatus) -> Result<(), StoreError> {
    let valid_hash =
        |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    let valid_text = |value: &str, maximum: usize| {
        !value.is_empty()
            && value.trim() == value
            && value.chars().count() <= maximum
            && !value.chars().any(char::is_control)
    };
    if !valid_hash(&status.channel_root_sha256)
        || !valid_hash(&status.archive_sha256)
        || !valid_text(&status.package_id, 512)
        || !valid_text(&status.package_version, 128)
        || status.revocation_reason.as_ref().is_some_and(|reason| !valid_text(reason, 64))
    {
        return Err(StoreError::InvalidChannelStatus);
    }
    Ok(())
}

fn ensure_active_package(
    context: &StoredContext,
    expected_package_sha256: &str,
) -> Result<(), StoreError> {
    if context.package_sha256 == expected_package_sha256 {
        Ok(())
    } else {
        Err(StoreError::ActiveContextChanged)
    }
}
fn validate_target_draft(target: &AnswerTarget) -> Result<(), StoreError> {
    let unique_aliases = target.aliases.iter().collect::<HashSet<_>>();
    let valid = !target.id.is_empty()
        && target.id.chars().count() <= MAX_IDENTIFIER_CHARS
        && !target.canonical.trim().is_empty()
        && target.canonical.chars().count() <= MAX_EXPRESSION_CHARS
        && target.aliases.len() <= MAX_ALIASES_PER_TARGET
        && unique_aliases.len() == target.aliases.len()
        && target
            .aliases
            .iter()
            .all(|alias| !alias.trim().is_empty() && alias.chars().count() <= MAX_EXPRESSION_CHARS);
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidTargetDraft(
            "identifier, canonical title, or aliases exceed the context limits",
        ))
    }
}

fn normalize_search(input: &str) -> String {
    input
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
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
