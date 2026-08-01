use std::{
    collections::VecDeque,
    fmt,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub use semantic_engine_audit_store::AuditEntry;
use semantic_engine_audit_store::{AuditError, AuditStore, RetentionPolicy};
use semantic_engine_core::{
    Decision, EvidenceKind, MAX_IDENTIFIER_CHARS, OperatorResolution, OperatorResolutionRequest,
    ResolutionIssue, Round, Submission, Validation, ValidationIssue, Validator, resolve_validation,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_RECORDED_VALIDATIONS: usize = 1_000_000;
const MAX_CACHE_CAPACITY: usize = 100_000;
const MAX_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_SESSIONS: usize = 10_000;
const MAX_EVENTS_PER_SESSION: usize = 100_000;
const MAX_EVENT_PAGE: usize = 1_000;
pub const SESSION_CONTRACT_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub max_recorded_validations: usize,
    pub cache_capacity: usize,
    pub cache_ttl: Duration,
    pub max_sessions: usize,
    pub max_events_per_session: usize,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            max_recorded_validations: 256,
            cache_capacity: 1_024,
            cache_ttl: Duration::from_secs(10 * 60),
            max_sessions: 128,
            max_events_per_session: 4_096,
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
    emitted_validation_ids: VecDeque<String>,
    emitted_resolution_ids: VecDeque<String>,
}

pub struct SemanticEngineService {
    audit: AuditStore,
    config: ServiceConfig,
    recorded: VecDeque<RecordedValidation>,
    cache: VecDeque<CacheEntry>,
    sessions: VecDeque<SessionRecord>,
    stats: ServiceStats,
}

impl SemanticEngineService {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ServiceError> {
        let audit = AuditStore::open(path, RetentionPolicy::default())?;
        Self::new(audit, ServiceConfig::default())
    }

    pub fn in_memory() -> Result<Self, ServiceError> {
        let audit = AuditStore::open_in_memory(RetentionPolicy::default())?;
        Self::new(audit, ServiceConfig::default())
    }

    pub fn in_memory_with_config(config: ServiceConfig) -> Result<Self, ServiceError> {
        let audit = AuditStore::open_in_memory(RetentionPolicy::default())?;
        Self::new(audit, config)
    }

    pub fn new(audit: AuditStore, config: ServiceConfig) -> Result<Self, ServiceError> {
        if config.max_recorded_validations == 0
            || config.max_recorded_validations > MAX_RECORDED_VALIDATIONS
            || config.cache_capacity > MAX_CACHE_CAPACITY
            || config.cache_ttl > MAX_CACHE_TTL
            || config.max_sessions == 0
            || config.max_sessions > MAX_SESSIONS
            || config.max_events_per_session == 0
            || config.max_events_per_session > MAX_EVENTS_PER_SESSION
        {
            return Err(ServiceError::InvalidConfig);
        }
        Ok(Self {
            audit,
            config,
            recorded: VecDeque::new(),
            cache: VecDeque::new(),
            sessions: VecDeque::new(),
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

        let cache_key = cache_fingerprint(&round, &submission.text, context_package_sha256);
        let validation = if let Some(mut cached) = self.take_cached(&cache_key) {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
            cached.round_id.clone_from(&round.id);
            cached.message_id.clone_from(&submission.message_id);
            cached.participant_id.clone_from(&submission.participant_id);
            cached.source_sequence = submission.source_sequence;
            cached
        } else {
            self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
            let validation = Validator::default().validate(&round, &submission);
            self.insert_cache(cache_key, validation.clone());
            validation
        };

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

    pub fn purge_audit(&mut self) -> Result<usize, ServiceError> {
        let deleted = self.audit.purge_all()?;
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
            emitted_validation_ids: VecDeque::new(),
            emitted_resolution_ids: VecDeque::new(),
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

    pub fn submit(
        &mut self,
        session_id: &str,
        submission: Submission,
    ) -> Result<Validation, ServiceError> {
        if !valid_session_identifier(session_id) {
            return Err(ServiceError::InvalidSession);
        }
        let (round, context_package_sha256) = {
            let session = self
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .ok_or(ServiceError::SessionMissing)?;
            if session.state == SessionState::Ended {
                return Err(ServiceError::SessionEnded);
            }
            (session.round.clone(), session.context_package_sha256.clone())
        };
        let validation = self.validate(round, submission, context_package_sha256.as_deref())?;
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
            .ok_or(ServiceError::SessionMissing)?;
        let already_emitted = session.emitted_validation_ids.contains(&validation.message_id);
        if !already_emitted {
            append_session_event(
                session,
                now_ms()?,
                SessionEventKind::ValidationRecorded(session_validation(&validation)),
                self.config.max_events_per_session,
            );
            remember_emitted_identity(
                &mut session.emitted_validation_ids,
                validation.message_id.clone(),
                self.config.max_recorded_validations,
            );
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
        let resolution = self.resolve(request)?;
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
            .ok_or(ServiceError::SessionMissing)?;
        let already_emitted = session.emitted_resolution_ids.contains(&resolution.message_id);
        if !already_emitted {
            append_session_event(
                session,
                now_ms()?,
                SessionEventKind::ResolutionRecorded(resolution.clone()),
                self.config.max_events_per_session,
            );
            remember_emitted_identity(
                &mut session.emitted_resolution_ids,
                resolution.message_id.clone(),
                self.config.max_recorded_validations,
            );
        }
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
        session.state = SessionState::Ended;
        session.ended_at_ms = Some(occurred_at_ms);
        append_session_event(
            session,
            occurred_at_ms,
            SessionEventKind::SessionEnded,
            self.config.max_events_per_session,
        );
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
    session.latest_event_sequence = session.latest_event_sequence.saturating_add(1);
    session.events.push_back(SessionEvent {
        contract_version: SESSION_CONTRACT_VERSION,
        session_id: session.session_id.clone(),
        sequence: session.latest_event_sequence,
        occurred_at_ms,
        kind,
    });
    while session.events.len() > max_events {
        session.events.pop_front();
    }
}

fn remember_emitted_identity(identities: &mut VecDeque<String>, identity: String, capacity: usize) {
    identities.push_back(identity);
    while identities.len() > capacity {
        identities.pop_front();
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
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"semantic-engine-cache-v1");
    hash_round(&mut hasher, round);
    hash_string(&mut hasher, submission_text);
    hash_optional_string(&mut hasher, context_package_sha256);
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
