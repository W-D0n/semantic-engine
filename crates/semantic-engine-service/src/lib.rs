use std::{
    collections::VecDeque,
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
pub use semantic_engine_audit_store::AuditEntry;
use semantic_engine_audit_store::{AuditError, AuditStore, RetentionPolicy};
use semantic_engine_core::{
    Decision, Evidence, EvidenceKind, MAX_EXPRESSION_CHARS, MAX_IDENTIFIER_CHARS,
    OperatorResolution, OperatorResolutionRequest, ResolutionIssue, Round, Submission, Validation,
    ValidationIssue, Validator, resolve_validation,
};
pub use semantic_engine_memory_store::{MemoryEntry, MemoryState};
use semantic_engine_memory_store::{MemoryError, MemoryPolicy, RecognitionMemoryStore};
use semantic_engine_session_store::{
    SessionStore, SessionStoreError, StoredDelivery, StoredEvent, StoredSessionHeader,
    StoredSessionState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_RECORDED_VALIDATIONS: usize = 1_000_000;
const MAX_CACHE_CAPACITY: usize = 100_000;
const MAX_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_SESSIONS: usize = 10_000;
const MAX_EVENTS_PER_SESSION: usize = 100_000;
const MAX_EVENT_PAGE: usize = 1_000;
pub const SESSION_CONTRACT_VERSION: u32 = 2;

#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub max_recorded_validations: usize,
    pub cache_capacity: usize,
    pub cache_ttl: Duration,
    pub max_sessions: usize,
    pub max_events_per_session: usize,
    pub memory_capacity: usize,
    pub memory_ttl: Duration,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            max_recorded_validations: 256,
            cache_capacity: 1_024,
            cache_ttl: Duration::from_secs(10 * 60),
            max_sessions: 128,
            max_events_per_session: 4_096,
            memory_capacity: 1_000,
            memory_ttl: Duration::from_secs(30 * 24 * 60 * 60),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ServiceStats {
    pub deduplicated: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_evictions: u64,
    pub cache_expirations: u64,
    pub cache_entries: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceError {
    InvalidConfig,
    IdentityConflict,
    ValidationMissing,
    SessionConflict,
    SessionMissing,
    SessionEnded,
    SessionCapacityExceeded,
    InvalidSession,
    Resolution(String),
    Audit(String),
    SessionStore(String),
    MemoryRejected(String),
    MemoryStore(String),
    Internal(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => write!(formatter, "service configuration is invalid"),
            Self::IdentityConflict => {
                write!(formatter, "submission identity already has different content")
            }
            Self::ValidationMissing => write!(formatter, "recorded validation does not exist"),
            Self::SessionConflict => {
                write!(formatter, "session identity already has different content")
            }
            Self::SessionMissing => write!(formatter, "session does not exist"),
            Self::SessionEnded => write!(formatter, "session has ended"),
            Self::SessionCapacityExceeded => {
                write!(formatter, "active session capacity is exhausted")
            }
            Self::InvalidSession => write!(formatter, "session definition is invalid"),
            Self::Resolution(message) => write!(formatter, "resolution was refused: {message}"),
            Self::Audit(message) => write!(formatter, "audit failed: {message}"),
            Self::SessionStore(message) => {
                write!(formatter, "session persistence failed: {message}")
            }
            Self::MemoryRejected(message) => {
                write!(formatter, "recognition memory request was refused: {message}")
            }
            Self::MemoryStore(message) => {
                write!(formatter, "recognition memory storage failed: {message}")
            }
            Self::Internal(message) => write!(formatter, "service internal error: {message}"),
        }
    }
}

impl std::error::Error for ServiceError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Active,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartSession {
    pub session_id: String,
    pub round: Round,
    pub context_package_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub contract_version: u32,
    pub session_id: String,
    pub round_id: String,
    pub context_package_sha256: Option<String>,
    pub state: SessionState,
    pub created_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub latest_event_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResumableSession {
    pub snapshot: SessionSnapshot,
    pub round: Round,
    pub next_source_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionValidation {
    pub round_id: String,
    pub message_id: String,
    pub participant_id: String,
    pub source_sequence: u64,
    pub decision: Decision,
    pub target_id: Option<String>,
    pub score: f64,
    pub evidence_kinds: Vec<EvidenceKind>,
    pub issue: Option<ValidationIssue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SessionEventKind {
    SessionStarted { round_id: String, context_package_sha256: Option<String> },
    ValidationRecorded(SessionValidation),
    ResolutionRecorded(OperatorResolution),
    SessionEnded,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    pub contract_version: u32,
    pub session_id: String,
    pub sequence: u64,
    pub occurred_at_ms: u64,
    #[serde(flatten)]
    pub kind: SessionEventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEventsPage {
    pub contract_version: u32,
    pub session_id: String,
    pub earliest_available_sequence: u64,
    pub latest_sequence: u64,
    pub truncated: bool,
    pub events: Vec<SessionEvent>,
}

impl From<AuditError> for ServiceError {
    fn from(error: AuditError) -> Self {
        Self::Audit(error.to_string())
    }
}

impl From<SessionStoreError> for ServiceError {
    fn from(error: SessionStoreError) -> Self {
        Self::SessionStore(error.to_string())
    }
}

impl From<MemoryError> for ServiceError {
    fn from(error: MemoryError) -> Self {
        match error {
            MemoryError::Database(_) | MemoryError::EntropyUnavailable => {
                Self::MemoryStore(error.to_string())
            }
            _ => Self::MemoryRejected(error.to_string()),
        }
    }
}

#[derive(Clone)]
struct RecordedValidation {
    request_fingerprint: [u8; 32],
    round: Round,
    validation: Validation,
}

#[derive(Clone)]
struct CacheEntry {
    key: [u8; 32],
    inserted_at: Instant,
    validation: Validation,
}

struct SessionRecord {
    definition_fingerprint: [u8; 32],
    session_id: String,
    round: Round,
    context_package_sha256: Option<String>,
    state: SessionState,
    created_at_ms: u64,
    ended_at_ms: Option<u64>,
    latest_event_sequence: u64,
    events: VecDeque<SessionEvent>,
    deliveries: VecDeque<SessionDelivery>,
}

struct SessionDelivery {
    request_fingerprint: [u8; 32],
    validation: SessionValidation,
    resolution_emitted: bool,
    submission_text: Option<String>,
}

pub struct SemanticEngineService {
    audit: AuditStore,
    session_store: SessionStore,
    memory: RecognitionMemoryStore,
    database_path: Option<PathBuf>,
    memory_provenance_key: [u8; 32],
    config: ServiceConfig,
    recorded: VecDeque<RecordedValidation>,
    cache: VecDeque<CacheEntry>,
    sessions: VecDeque<SessionRecord>,
    stats: ServiceStats,
}

impl SemanticEngineService {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ServiceError> {
        let path = path.as_ref();
        let config = ServiceConfig::default();
        validate_service_config(&config)?;
        let audit = AuditStore::open(path, RetentionPolicy::default())?;
        let session_store = SessionStore::open(path)?;
        let memory = RecognitionMemoryStore::open(path, memory_policy(&config))?;
        Self::new_with_stores(audit, session_store, memory, config, Some(path.to_path_buf()))
    }

    pub fn in_memory() -> Result<Self, ServiceError> {
        let audit = AuditStore::open_in_memory(RetentionPolicy::default())?;
        let session_store = SessionStore::open_in_memory()?;
        let config = ServiceConfig::default();
        validate_service_config(&config)?;
        let memory = RecognitionMemoryStore::open_in_memory(memory_policy(&config))?;
        Self::new_with_stores(audit, session_store, memory, config, None)
    }

    pub fn in_memory_with_config(config: ServiceConfig) -> Result<Self, ServiceError> {
        validate_service_config(&config)?;
        let audit = AuditStore::open_in_memory(RetentionPolicy::default())?;
        let session_store = SessionStore::open_in_memory()?;
        let memory = RecognitionMemoryStore::open_in_memory(memory_policy(&config))?;
        Self::new_with_stores(audit, session_store, memory, config, None)
    }

    pub fn new(audit: AuditStore, config: ServiceConfig) -> Result<Self, ServiceError> {
        validate_service_config(&config)?;
        let session_store = SessionStore::open_in_memory()?;
        let memory = RecognitionMemoryStore::open_in_memory(memory_policy(&config))?;
        Self::new_with_stores(audit, session_store, memory, config, None)
    }

    fn new_with_stores(
        audit: AuditStore,
        session_store: SessionStore,
        memory: RecognitionMemoryStore,
        config: ServiceConfig,
        database_path: Option<PathBuf>,
    ) -> Result<Self, ServiceError> {
        validate_service_config(&config)?;
        let sessions = restore_sessions(&session_store, &config)?;
        let mut memory_provenance_key = [0_u8; 32];
        getrandom::fill(&mut memory_provenance_key)
            .map_err(|_| ServiceError::Internal("OS randomness is unavailable".into()))?;
        Ok(Self {
            audit,
            session_store,
            memory,
            database_path,
            memory_provenance_key,
            config,
            recorded: VecDeque::new(),
            cache: VecDeque::new(),
            sessions,
            stats: ServiceStats::default(),
        })
    }

    pub fn validate(
        &mut self,
        round: Round,
        submission: Submission,
        context_package_sha256: Option<&str>,
    ) -> Result<Validation, ServiceError> {
        self.prune_expired_cache();
        let request_fingerprint = request_fingerprint(&round, &submission, context_package_sha256);
        if let Some(recorded) = self.recorded.iter().find(|recorded| {
            recorded.validation.round_id == round.id
                && recorded.validation.message_id == submission.message_id
        }) {
            if recorded.request_fingerprint != request_fingerprint {
                return Err(ServiceError::IdentityConflict);
            }
            self.stats.deduplicated = self.stats.deduplicated.saturating_add(1);
            return Ok(recorded.validation.clone());
        }

        let memory_matches =
            self.memory_matches(&round, &submission.text, context_package_sha256, now_ms()?)?;
        let cache_key =
            cache_fingerprint(&round, &submission.text, context_package_sha256, &memory_matches);
        let (computed, memory_applied) =
            validation_with_memory(&round, &submission, &memory_matches);
        let validation = if let Some(mut cached) = self.take_cached(&cache_key) {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
            cached.round_id.clone_from(&round.id);
            cached.message_id.clone_from(&submission.message_id);
            cached.participant_id.clone_from(&submission.participant_id);
            cached.source_sequence = submission.source_sequence;
            cached
        } else {
            self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
            self.insert_cache(cache_key, computed.clone());
            computed
        };

        if memory_applied && let Some(context) = context_package_sha256 {
            let memory_ids =
                memory_matches.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>();
            self.memory.mark_used(context, &memory_ids, now_ms()?)?;
        }

        self.audit.record_validation(&validation, context_package_sha256)?;
        self.recorded.push_back(RecordedValidation {
            request_fingerprint,
            round,
            validation: validation.clone(),
        });
        while self.recorded.len() > self.config.max_recorded_validations {
            self.recorded.pop_front();
        }
        Ok(validation)
    }

    pub fn resolve(
        &mut self,
        request: OperatorResolutionRequest,
    ) -> Result<OperatorResolution, ServiceError> {
        let recorded = self
            .recorded
            .iter()
            .find(|recorded| {
                recorded.validation.round_id == request.round_id
                    && recorded.validation.message_id == request.message_id
            })
            .ok_or(ServiceError::ValidationMissing)?;
        let resolution = resolve_validation(&recorded.round, &recorded.validation, request)
            .map_err(resolution_error)?;
        self.audit.record_resolution(&resolution)?;
        Ok(resolution)
    }

    pub fn recent_audit(&self, limit: usize) -> Result<Vec<AuditEntry>, ServiceError> {
        self.audit.recent(limit).map_err(Into::into)
    }

    pub fn purge_local_data(&mut self) -> Result<usize, ServiceError> {
        let deleted = if let Some(path) = &self.database_path {
            purge_persistent_data(path)?
        } else {
            self.session_store
                .purge_all()?
                .saturating_add(self.memory.purge_all()?)
                .saturating_add(self.audit.purge_all()?)
        };
        self.recorded.clear();
        self.cache.clear();
        self.sessions.clear();
        Ok(deleted)
    }

    pub fn stats(&self) -> ServiceStats {
        let mut stats = self.stats.clone();
        stats.cache_entries = self.cache.len();
        stats
    }

    pub fn remember_session_resolution(
        &mut self,
        session_id: &str,
        message_id: &str,
    ) -> Result<MemoryEntry, ServiceError> {
        if !valid_session_identifier(session_id) || !valid_session_identifier(message_id) {
            return Err(ServiceError::InvalidSession);
        }
        let (context, round_id, target_id, source_expression) = {
            let session = self
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .ok_or(ServiceError::SessionMissing)?;
            let context = session.context_package_sha256.clone().ok_or_else(|| {
                ServiceError::MemoryRejected("versioned context is required".into())
            })?;
            let resolution = session
                .events
                .iter()
                .rev()
                .find_map(|event| match &event.kind {
                    SessionEventKind::ResolutionRecorded(resolution)
                        if resolution.message_id == message_id =>
                    {
                        Some(resolution)
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    ServiceError::MemoryRejected("an operator resolution is required".into())
                })?;
            let source_expression = session
                .deliveries
                .iter()
                .find(|delivery| delivery.validation.message_id == message_id)
                .and_then(|delivery| delivery.submission_text.as_deref())
                .ok_or_else(|| {
                    ServiceError::MemoryRejected(
                        "the transient source expression is no longer available".into(),
                    )
                })?;
            if resolution.final_decision != Decision::Accepted {
                return Err(ServiceError::MemoryRejected(
                    "only accepted operator resolutions can be learned".into(),
                ));
            }
            let target_id = resolution.target_id.clone().ok_or_else(|| {
                ServiceError::MemoryRejected("accepted resolution has no target".into())
            })?;
            (context, session.round.id.clone(), target_id, source_expression.to_owned())
        };
        let source_resolution = memory_source_fingerprint(
            &self.memory_provenance_key,
            session_id,
            &round_id,
            message_id,
            &target_id,
        );
        self.memory
            .remember(&context, &target_id, &source_expression, &source_resolution, now_ms()?)
            .map_err(Into::into)
    }

    pub fn recognition_memory(
        &mut self,
        context_package_sha256: &str,
        limit: usize,
        active_only: bool,
    ) -> Result<Vec<MemoryEntry>, ServiceError> {
        if active_only {
            self.memory.list_active(context_package_sha256, limit, now_ms()?).map_err(Into::into)
        } else {
            self.memory.list(context_package_sha256, limit, now_ms()?).map_err(Into::into)
        }
    }

    pub fn revoke_memory(
        &mut self,
        context_package_sha256: &str,
        id: &str,
    ) -> Result<MemoryEntry, ServiceError> {
        self.memory.revoke(context_package_sha256, id, now_ms()?).map_err(Into::into)
    }

    fn memory_matches(
        &mut self,
        round: &Round,
        expression: &str,
        context_package_sha256: Option<&str>,
        current_time_ms: u64,
    ) -> Result<Vec<MemoryEntry>, ServiceError> {
        let Some(context) = context_package_sha256 else {
            return Ok(Vec::new());
        };
        if expression.is_empty()
            || expression.chars().count() > MAX_EXPRESSION_CHARS
            || expression.chars().any(char::is_control)
        {
            return Ok(Vec::new());
        }
        let target_ids = round.targets.iter().map(|target| target.id.clone()).collect::<Vec<_>>();
        self.memory.lookup(context, expression, &target_ids, current_time_ms).map_err(Into::into)
    }

    pub fn start_session(
        &mut self,
        request: StartSession,
    ) -> Result<SessionSnapshot, ServiceError> {
        validate_session_request(&request)?;
        let definition_fingerprint = session_definition_fingerprint(&request);
        if let Some(existing) =
            self.sessions.iter().find(|session| session.session_id == request.session_id)
        {
            if existing.definition_fingerprint != definition_fingerprint {
                return Err(ServiceError::SessionConflict);
            }
            return Ok(session_snapshot(existing));
        }

        if self.sessions.len() >= self.config.max_sessions {
            if let Some(position) =
                self.sessions.iter().position(|session| session.state == SessionState::Ended)
            {
                self.session_store.delete_ended(&self.sessions[position].session_id)?;
                self.sessions.remove(position);
            } else {
                return Err(ServiceError::SessionCapacityExceeded);
            }
        }

        let occurred_at_ms = now_ms()?;
        let mut session = SessionRecord {
            definition_fingerprint,
            session_id: request.session_id,
            round: request.round,
            context_package_sha256: request.context_package_sha256,
            state: SessionState::Active,
            created_at_ms: occurred_at_ms,
            ended_at_ms: None,
            latest_event_sequence: 0,
            events: VecDeque::new(),
            deliveries: VecDeque::new(),
        };
        let started_event = SessionEventKind::SessionStarted {
            round_id: session.round.id.clone(),
            context_package_sha256: session.context_package_sha256.clone(),
        };
        append_session_event(
            &mut session,
            occurred_at_ms,
            started_event,
            self.config.max_events_per_session,
        );
        let header = stored_session_header(&session)?;
        let event = session
            .events
            .back()
            .ok_or_else(|| ServiceError::Internal("started session event is missing".into()))?;
        self.session_store.create_session(&header, &stored_event(event)?)?;
        let snapshot = session_snapshot(&session);
        self.sessions.push_back(session);
        Ok(snapshot)
    }

    pub fn session(&self, session_id: &str) -> Result<SessionSnapshot, ServiceError> {
        if !valid_session_identifier(session_id) {
            return Err(ServiceError::InvalidSession);
        }
        self.sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .map(session_snapshot)
            .ok_or(ServiceError::SessionMissing)
    }

    pub fn latest_active_session(&self) -> Option<ResumableSession> {
        self.sessions
            .iter()
            .rev()
            .find(|session| session.state == SessionState::Active)
            .map(resumable_session)
    }

    pub fn resumable_session(&self, session_id: &str) -> Result<ResumableSession, ServiceError> {
        if !valid_session_identifier(session_id) {
            return Err(ServiceError::InvalidSession);
        }
        let session = self
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .ok_or(ServiceError::SessionMissing)?;
        if session.state == SessionState::Ended {
            return Err(ServiceError::SessionEnded);
        }
        Ok(resumable_session(session))
    }

    pub fn submit(
        &mut self,
        session_id: &str,
        submission: Submission,
    ) -> Result<Validation, ServiceError> {
        if !valid_session_identifier(session_id) {
            return Err(ServiceError::InvalidSession);
        }
        let (round, context_package_sha256, existing_delivery) = {
            let session = self
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .ok_or(ServiceError::SessionMissing)?;
            if session.state == SessionState::Ended {
                return Err(ServiceError::SessionEnded);
            }
            (
                session.round.clone(),
                session.context_package_sha256.clone(),
                session
                    .deliveries
                    .iter()
                    .find(|delivery| delivery.validation.message_id == submission.message_id)
                    .map(|delivery| (delivery.request_fingerprint, delivery.validation.clone())),
            )
        };
        let fingerprint =
            request_fingerprint(&round, &submission, context_package_sha256.as_deref());
        if let Some((existing_fingerprint, _)) = existing_delivery {
            if existing_fingerprint != fingerprint {
                return Err(ServiceError::IdentityConflict);
            }
            self.stats.deduplicated = self.stats.deduplicated.saturating_add(1);
            return Ok(Validator::default().validate(&round, &submission));
        }

        let submission_text = submission.text.clone();
        let validation = self.validate(round, submission, context_package_sha256.as_deref())?;
        let occurred_at_ms = now_ms()?;
        let validation_summary = session_validation(&validation);
        let (event, delivery) = {
            let session = self
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .ok_or(ServiceError::SessionMissing)?;
            let event = new_session_event(
                session,
                occurred_at_ms,
                SessionEventKind::ValidationRecorded(validation_summary.clone()),
            );
            let delivery = SessionDelivery {
                request_fingerprint: fingerprint,
                validation: validation_summary,
                resolution_emitted: false,
                submission_text: Some(submission_text),
            };
            (event, delivery)
        };
        self.session_store.record_validation(
            session_id,
            &stored_event(&event)?,
            &stored_delivery(&delivery, event.sequence)?,
            self.config.max_events_per_session,
            self.config.max_recorded_validations,
        )?;
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
            .ok_or(ServiceError::SessionMissing)?;
        push_session_event(session, event, self.config.max_events_per_session);
        session.deliveries.push_back(delivery);
        while session.deliveries.len() > self.config.max_recorded_validations {
            session.deliveries.pop_front();
        }
        Ok(validation)
    }

    pub fn resolve_session(
        &mut self,
        session_id: &str,
        request: OperatorResolutionRequest,
    ) -> Result<OperatorResolution, ServiceError> {
        if !valid_session_identifier(session_id) {
            return Err(ServiceError::InvalidSession);
        }
        let (round, validation, already_emitted) = {
            let session = self
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .ok_or(ServiceError::SessionMissing)?;
            if session.state == SessionState::Ended {
                return Err(ServiceError::SessionEnded);
            }
            if session.round.id != request.round_id {
                return Err(ServiceError::SessionConflict);
            }
            let delivery = session
                .deliveries
                .iter()
                .find(|delivery| delivery.validation.message_id == request.message_id)
                .ok_or(ServiceError::ValidationMissing)?;
            (
                session.round.clone(),
                validation_from_session(&delivery.validation),
                delivery.resolution_emitted,
            )
        };
        let resolution =
            resolve_validation(&round, &validation, request).map_err(resolution_error)?;
        self.audit.record_resolution(&resolution)?;
        if already_emitted {
            return Ok(resolution);
        }
        let event = {
            let session = self
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .ok_or(ServiceError::SessionMissing)?;
            new_session_event(
                session,
                now_ms()?,
                SessionEventKind::ResolutionRecorded(resolution.clone()),
            )
        };
        self.session_store.record_resolution(
            session_id,
            &resolution.message_id,
            &stored_event(&event)?,
            self.config.max_events_per_session,
        )?;
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
            .ok_or(ServiceError::SessionMissing)?;
        push_session_event(session, event, self.config.max_events_per_session);
        let delivery = session
            .deliveries
            .iter_mut()
            .find(|delivery| delivery.validation.message_id == resolution.message_id)
            .ok_or(ServiceError::ValidationMissing)?;
        delivery.resolution_emitted = true;
        Ok(resolution)
    }

    pub fn end_session(&mut self, session_id: &str) -> Result<SessionSnapshot, ServiceError> {
        if !valid_session_identifier(session_id) {
            return Err(ServiceError::InvalidSession);
        }
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
            .ok_or(ServiceError::SessionMissing)?;
        if session.state == SessionState::Ended {
            return Ok(session_snapshot(session));
        }
        let occurred_at_ms = now_ms()?;
        let event = new_session_event(session, occurred_at_ms, SessionEventKind::SessionEnded);
        self.session_store.end_session(
            session_id,
            &stored_event(&event)?,
            self.config.max_events_per_session,
        )?;
        session.state = SessionState::Ended;
        session.ended_at_ms = Some(occurred_at_ms);
        push_session_event(session, event, self.config.max_events_per_session);
        Ok(session_snapshot(session))
    }

    pub fn session_events(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: usize,
    ) -> Result<SessionEventsPage, ServiceError> {
        if !valid_session_identifier(session_id) || limit > MAX_EVENT_PAGE {
            return Err(ServiceError::InvalidSession);
        }
        let session = self
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .ok_or(ServiceError::SessionMissing)?;
        let earliest_available_sequence = session
            .events
            .front()
            .map_or(session.latest_event_sequence.saturating_add(1), |event| event.sequence);
        let events = session
            .events
            .iter()
            .filter(|event| event.sequence > after_sequence)
            .take(limit)
            .cloned()
            .collect();
        Ok(SessionEventsPage {
            contract_version: SESSION_CONTRACT_VERSION,
            session_id: session.session_id.clone(),
            earliest_available_sequence,
            latest_sequence: session.latest_event_sequence,
            truncated: after_sequence.saturating_add(1) < earliest_available_sequence,
            events,
        })
    }

    fn take_cached(&mut self, key: &[u8; 32]) -> Option<Validation> {
        let position = self.cache.iter().position(|entry| &entry.key == key)?;
        let entry = self.cache.remove(position)?;
        if entry.inserted_at.elapsed() >= self.config.cache_ttl {
            self.stats.cache_expirations = self.stats.cache_expirations.saturating_add(1);
            return None;
        }
        let validation = entry.validation.clone();
        self.cache.push_back(entry);
        Some(validation)
    }

    fn prune_expired_cache(&mut self) {
        let before = self.cache.len();
        self.cache.retain(|entry| entry.inserted_at.elapsed() < self.config.cache_ttl);
        let expired = before.saturating_sub(self.cache.len());
        self.stats.cache_expirations =
            self.stats.cache_expirations.saturating_add(u64::try_from(expired).unwrap_or(u64::MAX));
    }

    fn insert_cache(&mut self, key: [u8; 32], validation: Validation) {
        if self.config.cache_capacity == 0 {
            return;
        }
        if let Some(position) = self.cache.iter().position(|entry| entry.key == key) {
            self.cache.remove(position);
        }
        self.cache.push_back(CacheEntry { key, inserted_at: Instant::now(), validation });
        while self.cache.len() > self.config.cache_capacity {
            self.cache.pop_front();
            self.stats.cache_evictions = self.stats.cache_evictions.saturating_add(1);
        }
    }
}

fn resumable_session(session: &SessionRecord) -> ResumableSession {
    ResumableSession {
        snapshot: session_snapshot(session),
        round: session.round.clone(),
        next_source_sequence: session
            .deliveries
            .iter()
            .map(|delivery| delivery.validation.source_sequence)
            .max()
            .map_or(0, |sequence| sequence.saturating_add(1)),
    }
}

fn validate_session_request(request: &StartSession) -> Result<(), ServiceError> {
    if !valid_session_identifier(&request.session_id)
        || request.context_package_sha256.as_deref().is_some_and(|value| {
            value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(ServiceError::InvalidSession);
    }
    let preflight = Validator::default().validate(
        &request.round,
        &Submission {
            message_id: "session-preflight".to_owned(),
            participant_id: "session-preflight".to_owned(),
            source_sequence: 0,
            text: "session preflight".to_owned(),
        },
    );
    if matches!(
        preflight.issue,
        Some(ValidationIssue::InvalidRound | ValidationIssue::InvalidPolicy)
    ) {
        return Err(ServiceError::InvalidSession);
    }
    Ok(())
}

fn valid_session_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_IDENTIFIER_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn session_snapshot(session: &SessionRecord) -> SessionSnapshot {
    SessionSnapshot {
        contract_version: SESSION_CONTRACT_VERSION,
        session_id: session.session_id.clone(),
        round_id: session.round.id.clone(),
        context_package_sha256: session.context_package_sha256.clone(),
        state: session.state.clone(),
        created_at_ms: session.created_at_ms,
        ended_at_ms: session.ended_at_ms,
        latest_event_sequence: session.latest_event_sequence,
    }
}

fn session_validation(validation: &Validation) -> SessionValidation {
    SessionValidation {
        round_id: validation.round_id.clone(),
        message_id: validation.message_id.clone(),
        participant_id: validation.participant_id.clone(),
        source_sequence: validation.source_sequence,
        decision: validation.decision.clone(),
        target_id: validation.target_id.clone(),
        score: validation.score,
        evidence_kinds: validation.evidence.iter().map(|item| item.kind.clone()).collect(),
        issue: validation.issue.clone(),
    }
}

fn append_session_event(
    session: &mut SessionRecord,
    occurred_at_ms: u64,
    kind: SessionEventKind,
    max_events: usize,
) {
    let event = new_session_event(session, occurred_at_ms, kind);
    push_session_event(session, event, max_events);
}

fn new_session_event(
    session: &SessionRecord,
    occurred_at_ms: u64,
    kind: SessionEventKind,
) -> SessionEvent {
    SessionEvent {
        contract_version: SESSION_CONTRACT_VERSION,
        session_id: session.session_id.clone(),
        sequence: session.latest_event_sequence.saturating_add(1),
        occurred_at_ms,
        kind,
    }
}

fn push_session_event(session: &mut SessionRecord, event: SessionEvent, max_events: usize) {
    session.latest_event_sequence = event.sequence;
    session.events.push_back(event);
    while session.events.len() > max_events {
        session.events.pop_front();
    }
}

fn stored_session_header(session: &SessionRecord) -> Result<StoredSessionHeader, ServiceError> {
    let definition = StartSession {
        session_id: session.session_id.clone(),
        round: session.round.clone(),
        context_package_sha256: session.context_package_sha256.clone(),
    };
    Ok(StoredSessionHeader {
        session_id: session.session_id.clone(),
        definition_fingerprint: session.definition_fingerprint,
        definition_json: serde_json::to_string(&definition)
            .map_err(|error| ServiceError::Internal(error.to_string()))?,
        state: match session.state {
            SessionState::Active => StoredSessionState::Active,
            SessionState::Ended => StoredSessionState::Ended,
        },
        created_at_ms: session.created_at_ms,
        ended_at_ms: session.ended_at_ms,
        latest_event_sequence: session.latest_event_sequence,
    })
}

fn stored_event(event: &SessionEvent) -> Result<StoredEvent, ServiceError> {
    Ok(StoredEvent {
        sequence: event.sequence,
        occurred_at_ms: event.occurred_at_ms,
        payload_json: serde_json::to_string(event)
            .map_err(|error| ServiceError::Internal(error.to_string()))?,
    })
}

fn stored_delivery(
    delivery: &SessionDelivery,
    sequence: u64,
) -> Result<StoredDelivery, ServiceError> {
    Ok(StoredDelivery {
        message_id: delivery.validation.message_id.clone(),
        sequence,
        request_fingerprint: delivery.request_fingerprint,
        validation_json: serde_json::to_string(&delivery.validation)
            .map_err(|error| ServiceError::Internal(error.to_string()))?,
        resolution_emitted: delivery.resolution_emitted,
    })
}

fn restore_sessions(
    store: &SessionStore,
    config: &ServiceConfig,
) -> Result<VecDeque<SessionRecord>, ServiceError> {
    let stored = store.load_sessions()?;
    if stored.len() > config.max_sessions {
        return Err(ServiceError::SessionCapacityExceeded);
    }
    stored
        .into_iter()
        .map(|stored| {
            let definition: StartSession = serde_json::from_str(&stored.header.definition_json)
                .map_err(|error| ServiceError::SessionStore(error.to_string()))?;
            validate_session_request(&definition)?;
            if definition.session_id != stored.header.session_id
                || session_definition_fingerprint(&definition)
                    != stored.header.definition_fingerprint
            {
                return Err(ServiceError::SessionStore(
                    "durable session definition fingerprint is invalid".into(),
                ));
            }
            let mut events = stored
                .events
                .into_iter()
                .map(|item| {
                    let mut event: SessionEvent = serde_json::from_str(&item.payload_json)
                        .map_err(|error| ServiceError::SessionStore(error.to_string()))?;
                    if event.session_id != definition.session_id
                        || event.sequence != item.sequence
                        || event.occurred_at_ms != item.occurred_at_ms
                        || !matches!(event.contract_version, 1 | SESSION_CONTRACT_VERSION)
                    {
                        return Err(ServiceError::SessionStore(
                            "durable session event identity is invalid".into(),
                        ));
                    }
                    event.contract_version = SESSION_CONTRACT_VERSION;
                    Ok(event)
                })
                .collect::<Result<VecDeque<_>, _>>()?;
            while events.len() > config.max_events_per_session {
                events.pop_front();
            }
            let mut deliveries = stored
                .deliveries
                .into_iter()
                .map(|item| {
                    let validation: SessionValidation = serde_json::from_str(&item.validation_json)
                        .map_err(|error| ServiceError::SessionStore(error.to_string()))?;
                    if validation.message_id != item.message_id {
                        return Err(ServiceError::SessionStore(
                            "durable delivery identity is invalid".into(),
                        ));
                    }
                    Ok(SessionDelivery {
                        request_fingerprint: item.request_fingerprint,
                        validation,
                        resolution_emitted: item.resolution_emitted,
                        submission_text: None,
                    })
                })
                .collect::<Result<VecDeque<_>, _>>()?;
            while deliveries.len() > config.max_recorded_validations {
                deliveries.pop_front();
            }
            let state = match stored.header.state {
                StoredSessionState::Active => SessionState::Active,
                StoredSessionState::Ended => SessionState::Ended,
            };
            Ok(SessionRecord {
                definition_fingerprint: stored.header.definition_fingerprint,
                session_id: definition.session_id,
                round: definition.round,
                context_package_sha256: definition.context_package_sha256,
                state,
                created_at_ms: stored.header.created_at_ms,
                ended_at_ms: stored.header.ended_at_ms,
                latest_event_sequence: stored.header.latest_event_sequence,
                events,
                deliveries,
            })
        })
        .collect()
}

fn validation_from_session(validation: &SessionValidation) -> Validation {
    Validation {
        round_id: validation.round_id.clone(),
        message_id: validation.message_id.clone(),
        participant_id: validation.participant_id.clone(),
        source_sequence: validation.source_sequence,
        decision: validation.decision.clone(),
        target_id: validation.target_id.clone(),
        score: validation.score,
        evidence: validation
            .evidence_kinds
            .iter()
            .cloned()
            .map(|kind| Evidence { kind, matched_expression: String::new() })
            .collect(),
        issue: validation.issue.clone(),
    }
}

fn session_definition_fingerprint(request: &StartSession) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"semantic-engine-session-v1");
    hash_string(&mut hasher, &request.session_id);
    hash_round(&mut hasher, &request.round);
    hash_optional_string(&mut hasher, request.context_package_sha256.as_deref());
    hasher.finalize().into()
}

fn now_ms() -> Result<u64, ServiceError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ServiceError::Internal(error.to_string()))?;
    u64::try_from(duration.as_millis())
        .map_err(|_| ServiceError::Internal("system time does not fit u64 milliseconds".to_owned()))
}

fn validate_service_config(config: &ServiceConfig) -> Result<(), ServiceError> {
    if config.max_recorded_validations == 0
        || config.max_recorded_validations > MAX_RECORDED_VALIDATIONS
        || config.cache_capacity > MAX_CACHE_CAPACITY
        || config.cache_ttl > MAX_CACHE_TTL
        || config.max_sessions == 0
        || config.max_sessions > MAX_SESSIONS
        || config.max_events_per_session == 0
        || config.max_events_per_session > MAX_EVENTS_PER_SESSION
        || memory_policy(config).validate().is_err()
    {
        return Err(ServiceError::InvalidConfig);
    }
    Ok(())
}

fn memory_policy(config: &ServiceConfig) -> MemoryPolicy {
    MemoryPolicy { capacity: config.memory_capacity, ttl: config.memory_ttl }
}

fn validation_with_memory(
    round: &Round,
    submission: &Submission,
    memory_matches: &[MemoryEntry],
) -> (Validation, bool) {
    let configured = Validator::default().validate(round, submission);
    let configured_exact = configured.evidence.iter().any(|evidence| {
        matches!(
            evidence.kind,
            EvidenceKind::ConfiguredExpression
                | EvidenceKind::NormalizedExpression
                | EvidenceKind::AmbiguousExpression
        ) && configured.score == 1.0
    });
    if configured_exact || memory_matches.is_empty() || configured.issue.is_some() {
        return (configured, false);
    }

    let first_target = &memory_matches[0].target_id;
    let ambiguous = memory_matches.iter().any(|entry| entry.target_id != *first_target);
    let validation = Validation {
        round_id: round.id.clone(),
        message_id: submission.message_id.clone(),
        participant_id: submission.participant_id.clone(),
        source_sequence: submission.source_sequence,
        decision: if ambiguous { Decision::Abstained } else { Decision::Accepted },
        target_id: (!ambiguous).then(|| first_target.clone()),
        score: 1.0,
        evidence: vec![Evidence {
            kind: if ambiguous {
                EvidenceKind::AmbiguousExpression
            } else {
                EvidenceKind::MemoryExpression
            },
            matched_expression: memory_matches[0].expression.clone(),
        }],
        issue: None,
    };
    (validation, true)
}

fn purge_persistent_data(path: &Path) -> Result<usize, ServiceError> {
    let mut connection =
        Connection::open(path).map_err(|error| ServiceError::Internal(error.to_string()))?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|error| ServiceError::Internal(error.to_string()))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")
        .map_err(|error| ServiceError::Internal(error.to_string()))?;
    let transaction =
        connection.transaction().map_err(|error| ServiceError::Internal(error.to_string()))?;
    let session_deleted = transaction
        .execute("DELETE FROM session_records", [])
        .map_err(|error| ServiceError::Internal(error.to_string()))?;
    let memory_deleted = transaction
        .execute("DELETE FROM recognition_memory", [])
        .map_err(|error| ServiceError::Internal(error.to_string()))?;
    let deleted = transaction
        .execute("DELETE FROM audit_validations", [])
        .map_err(|error| ServiceError::Internal(error.to_string()))?;
    transaction.commit().map_err(|error| ServiceError::Internal(error.to_string()))?;
    let total = session_deleted.saturating_add(memory_deleted).saturating_add(deleted);
    if total > 0 {
        let _ = connection.execute_batch("VACUUM;");
    }
    Ok(total)
}

fn memory_source_fingerprint(
    key: &[u8; 32],
    session_id: &str,
    round_id: &str,
    message_id: &str,
    target_id: &str,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"semantic-engine-operator-memory-v1");
    hasher.update(key);
    hash_string(&mut hasher, session_id);
    hash_string(&mut hasher, round_id);
    hash_string(&mut hasher, message_id);
    hash_string(&mut hasher, target_id);
    hasher.finalize().into()
}

fn request_fingerprint(
    round: &Round,
    submission: &Submission,
    context_package_sha256: Option<&str>,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"semantic-engine-request-v1");
    hash_round(&mut hasher, round);
    hash_string(&mut hasher, &submission.message_id);
    hash_string(&mut hasher, &submission.participant_id);
    hasher.update(submission.source_sequence.to_le_bytes());
    hash_string(&mut hasher, &submission.text);
    hash_optional_string(&mut hasher, context_package_sha256);
    hasher.finalize().into()
}

fn cache_fingerprint(
    round: &Round,
    submission_text: &str,
    context_package_sha256: Option<&str>,
    memory_matches: &[MemoryEntry],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"semantic-engine-cache-v1");
    hash_round(&mut hasher, round);
    hash_string(&mut hasher, submission_text);
    hash_optional_string(&mut hasher, context_package_sha256);
    hash_usize(&mut hasher, memory_matches.len());
    for entry in memory_matches {
        hash_string(&mut hasher, &entry.id);
        hash_string(&mut hasher, &entry.target_id);
    }
    hasher.finalize().into()
}

fn hash_round(hasher: &mut Sha256, round: &Round) {
    hash_string(hasher, &round.id);
    hash_usize(hasher, round.targets.len());
    for target in &round.targets {
        hash_string(hasher, &target.id);
        hash_string(hasher, &target.canonical);
        hash_usize(hasher, target.aliases.len());
        for alias in &target.aliases {
            hash_string(hasher, alias);
        }
    }
    hasher.update(round.policy.accept_threshold.to_bits().to_le_bytes());
    hasher.update(round.policy.review_threshold.to_bits().to_le_bytes());
    hasher.update(round.policy.ambiguity_margin.to_bits().to_le_bytes());
}

fn hash_optional_string(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_string(hasher, value);
        }
        None => hasher.update([0]),
    }
}

fn hash_string(hasher: &mut Sha256, value: &str) {
    hash_usize(hasher, value.len());
    hasher.update(value.as_bytes());
}

fn hash_usize(hasher: &mut Sha256, value: usize) {
    hasher.update(u64::try_from(value).unwrap_or(u64::MAX).to_le_bytes());
}

fn resolution_error(error: ResolutionIssue) -> ServiceError {
    ServiceError::Resolution(format!("{error:?}"))
}
