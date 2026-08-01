use std::time::Duration;

use semantic_engine_audit_store::{AuditStore, RetentionPolicy};
use semantic_engine_core::{
    AnswerTarget, Decision, OperatorResolutionRequest, ResolutionVerdict, Round, Submission,
    ValidationIssue, ValidationPolicy,
};
use semantic_engine_service::{SemanticEngineService, ServiceConfig, ServiceError};

fn service(cache_capacity: usize, cache_ttl: Duration) -> SemanticEngineService {
    SemanticEngineService::new(
        AuditStore::open_in_memory(RetentionPolicy::default()).expect("audit store"),
        ServiceConfig { max_recorded_validations: 8, cache_capacity, cache_ttl },
    )
    .expect("service")
}

fn round() -> Round {
    Round {
        id: "round-1".to_owned(),
        targets: vec![AnswerTarget {
            id: "elden-ring".to_owned(),
            canonical: "Elden Ring".to_owned(),
            aliases: vec!["ER".to_owned()],
        }],
        policy: ValidationPolicy::default(),
    }
}

fn submission(message_id: &str, participant_id: &str, sequence: u64, text: &str) -> Submission {
    Submission {
        message_id: message_id.to_owned(),
        participant_id: participant_id.to_owned(),
        source_sequence: sequence,
        text: text.to_owned(),
    }
}

#[test]
fn identity_retry_is_idempotent_but_conflicting_content_is_rejected() {
    let mut service = service(4, Duration::from_secs(60));
    let request = submission("message-1", "viewer-1", 1, "Elden Ring");
    let first = service.validate(round(), request.clone(), None).expect("first validation");
    let duplicate = service.validate(round(), request, None).expect("idempotent retry");
    assert_eq!(duplicate, first);
    assert_eq!(service.stats().deduplicated, 1);

    let conflict =
        service.validate(round(), submission("message-1", "viewer-1", 1, "Dark Souls"), None);
    assert_eq!(conflict, Err(ServiceError::IdentityConflict));
}

#[test]
fn cache_is_bounded_and_partitioned_by_context_version() {
    let mut service = service(2, Duration::from_secs(60));
    let context_a = "a".repeat(64);
    let context_b = "b".repeat(64);

    service
        .validate(round(), submission("m1", "v1", 1, "Elden Ring"), Some(&context_a))
        .expect("first miss");
    service
        .validate(round(), submission("m2", "v2", 2, "Elden Ring"), Some(&context_a))
        .expect("cache hit");
    service
        .validate(round(), submission("m3", "v3", 3, "Elden Ring"), Some(&context_b))
        .expect("other context miss");
    service
        .validate(round(), submission("m4", "v4", 4, "elden ring"), Some(&context_a))
        .expect("second expression miss and eviction");

    let stats = service.stats();
    assert_eq!(stats.cache_hits, 1);
    assert_eq!(stats.cache_misses, 3);
    assert_eq!(stats.cache_evictions, 1);
    assert_eq!(stats.cache_entries, 2);
}

#[test]
fn expired_cache_entry_is_recomputed_without_sleeping() {
    let mut service = service(2, Duration::ZERO);
    service.validate(round(), submission("m1", "v1", 1, "Elden Ring"), None).expect("first miss");
    service.validate(round(), submission("m2", "v2", 2, "Elden Ring"), None).expect("expired miss");
    assert_eq!(service.stats().cache_hits, 0);
    assert_eq!(service.stats().cache_misses, 2);
    assert_eq!(service.stats().cache_expirations, 1);
}

#[test]
fn zero_capacity_explicitly_disables_cache_storage() {
    let mut service = service(0, Duration::from_secs(60));
    service.validate(round(), submission("m1", "v1", 1, "Elden Ring"), None).expect("first miss");
    service.validate(round(), submission("m2", "v2", 2, "Elden Ring"), None).expect("second miss");
    assert_eq!(service.stats().cache_entries, 0);
    assert_eq!(service.stats().cache_hits, 0);
    assert_eq!(service.stats().cache_misses, 2);
}

#[test]
fn invalid_non_finite_policy_still_returns_the_core_validation_issue() {
    let mut service = service(2, Duration::from_secs(60));
    let mut invalid_round = round();
    invalid_round.policy.accept_threshold = f64::NAN;
    let validation = service
        .validate(invalid_round, submission("m1", "v1", 1, "Elden Ring"), None)
        .expect("invalid policy is a validation result");
    assert_eq!(validation.issue, Some(ValidationIssue::InvalidPolicy));
}

#[test]
fn resolution_and_purge_flow_through_the_shared_service() {
    let mut service = service(4, Duration::from_secs(60));
    let validation = service
        .validate(round(), submission("m1", "v1", 1, "elden kings"), None)
        .expect("validation");
    let resolution = service
        .resolve(OperatorResolutionRequest {
            round_id: "round-1".to_owned(),
            message_id: "m1".to_owned(),
            verdict: ResolutionVerdict::Rejected,
            target_id: None,
            note: "operator decision".to_owned(),
        })
        .expect("resolution");
    assert_eq!(resolution.original_decision, validation.decision);
    assert_eq!(resolution.final_decision, Decision::Rejected);
    assert!(service.recent_audit(8).expect("audit")[0].resolution.is_some());

    assert_eq!(service.purge_audit().expect("purge"), 1);
    assert!(service.recent_audit(8).expect("empty audit").is_empty());
    assert_eq!(
        service.resolve(OperatorResolutionRequest {
            round_id: "round-1".to_owned(),
            message_id: "m1".to_owned(),
            verdict: ResolutionVerdict::Rejected,
            target_id: None,
            note: String::new(),
        }),
        Err(ServiceError::ValidationMissing)
    );
}
