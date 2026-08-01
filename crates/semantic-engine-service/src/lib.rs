use std::{collections::VecDeque, fmt, path::Path, time::Duration, time::Instant};

pub use semantic_engine_audit_store::AuditEntry;
use semantic_engine_audit_store::{AuditError, AuditStore, RetentionPolicy};
use semantic_engine_core::{
    OperatorResolution, OperatorResolutionRequest, ResolutionIssue, Round, Submission, Validation,
    Validator, resolve_validation,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

const MAX_RECORDED_VALIDATIONS: usize = 1_000_000;
const MAX_CACHE_CAPACITY: usize = 100_000;
const MAX_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub max_recorded_validations: usize,
    pub cache_capacity: usize,
    pub cache_ttl: Duration,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            max_recorded_validations: 256,
            cache_capacity: 1_024,
            cache_ttl: Duration::from_secs(10 * 60),
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
            Self::Resolution(message) => write!(formatter, "resolution was refused: {message}"),
            Self::Audit(message) => write!(formatter, "audit failed: {message}"),
            Self::Internal(message) => write!(formatter, "service internal error: {message}"),
        }
    }
}

impl std::error::Error for ServiceError {}

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

pub struct SemanticEngineService {
    audit: AuditStore,
    config: ServiceConfig,
    recorded: VecDeque<RecordedValidation>,
    cache: VecDeque<CacheEntry>,
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
        {
            return Err(ServiceError::InvalidConfig);
        }
        Ok(Self {
            audit,
            config,
            recorded: VecDeque::new(),
            cache: VecDeque::new(),
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
        Ok(deleted)
    }

    pub fn stats(&self) -> ServiceStats {
        let mut stats = self.stats.clone();
        stats.cache_entries = self.cache.len();
        stats
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
