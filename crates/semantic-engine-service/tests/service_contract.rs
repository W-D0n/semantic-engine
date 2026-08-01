use std::time::Duration;

use semantic_engine_audit_store::{AuditStore, RetentionPolicy};
use semantic_engine_core::{
    AnswerTarget, Decision, OperatorResolutionRequest, ResolutionVerdict, Round, Submission,
    ValidationIssue, ValidationPolicy,
};
use semantic_engine_service::{
    SemanticEngineService, ServiceConfig, ServiceError, SessionEventKind, SessionState,
    StartSession,
};

fn service(cache_capacity: usize, cache_ttl: Duration) -> SemanticEngineService {
    SemanticEngineService::new(
        AuditStore::open_in_memory(RetentionPolicy::default()).expect("audit store"),
        ServiceConfig {
            max_recorded_validations: 8,
            cache_capacity,
            cache_ttl,
            max_sessions: 4,
            max_events_per_session: 4,
        },
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
    service
        .start_session(StartSession {
            session_id: "purged-session".to_owned(),
            round: round(),
            context_package_sha256: None,
        })
        .expect("session before purge");
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
    assert_eq!(service.session("purged-session"), Err(ServiceError::SessionMissing));
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

#[test]
fn session_lifecycle_binds_round_context_and_privacy_minimized_events() {
    let mut service = service(4, Duration::from_secs(60));
    let context_sha = "a".repeat(64);
    let started = service
        .start_session(StartSession {
            session_id: "session-1".to_owned(),
            round: round(),
            context_package_sha256: Some(context_sha.clone()),
        })
        .expect("start session");
    assert_eq!(started.state, SessionState::Active);
    assert_eq!(started.context_package_sha256.as_deref(), Some(context_sha.as_str()));

    let validation = service
        .submit("session-1", submission("message-1", "viewer-1", 1, "Elden Ring"))
        .expect("session submission");
    assert_eq!(validation.decision, Decision::Accepted);

    let page = service.session_events("session-1", 0, 10).expect("session events");
    assert_eq!(page.events.len(), 2);
    let serialized = serde_json::to_string(&page).expect("serialize event page");
    assert!(!serialized.contains("Elden Ring"));
    assert!(matches!(page.events[0].kind, SessionEventKind::SessionStarted { .. }));
    assert!(matches!(page.events[1].kind, SessionEventKind::ValidationRecorded(_)));

    let ended = service.end_session("session-1").expect("end session");
    assert_eq!(ended.state, SessionState::Ended);
    assert_eq!(service.end_session("session-1").expect("idempotent end"), ended);
    assert_eq!(
        service.submit("session-1", submission("message-2", "viewer-2", 2, "ER")),
        Err(ServiceError::SessionEnded)
    );
}

#[test]
fn session_start_is_idempotent_and_conflicting_redefinition_is_rejected() {
    let mut service = service(4, Duration::from_secs(60));
    let request = StartSession {
        session_id: "session-1".to_owned(),
        round: round(),
        context_package_sha256: None,
    };
    let first = service.start_session(request.clone()).expect("first start");
    assert_eq!(service.start_session(request).expect("idempotent start"), first);

    let mut other_round = round();
    other_round.id = "other-round".to_owned();
    assert_eq!(
        service.start_session(StartSession {
            session_id: "session-1".to_owned(),
            round: other_round,
            context_package_sha256: None,
        }),
        Err(ServiceError::SessionConflict)
    );
}

#[test]
fn bounded_event_page_reports_a_gap_instead_of_hiding_loss() {
    let mut service = service(4, Duration::from_secs(60));
    service
        .start_session(StartSession {
            session_id: "session-1".to_owned(),
            round: round(),
            context_package_sha256: None,
        })
        .expect("start");
    for sequence in 1..=5 {
        service
            .submit(
                "session-1",
                submission(&format!("message-{sequence}"), "viewer", sequence, "Elden Ring"),
            )
            .expect("submit");
    }

    let page = service.session_events("session-1", 0, 10).expect("events");
    assert!(page.truncated);
    assert_eq!(page.events.len(), 4);
    assert_eq!(page.earliest_available_sequence, 3);
    assert_eq!(page.latest_sequence, 6);

    service
        .submit("session-1", submission("message-1", "viewer", 1, "Elden Ring"))
        .expect("deduplicated submit");
    let deduplicated = service.session_events("session-1", 0, 10).expect("events");
    assert_eq!(deduplicated.latest_sequence, 6);
}

#[test]
fn active_session_idempotence_and_resolution_survive_service_restart() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("semantic-engine.sqlite3");
    let private_text = "elden kings PRIVATE-CHAT-MARKER";
    let first_validation;

    {
        let mut service = SemanticEngineService::open(&database).expect("open service");
        service
            .start_session(StartSession {
                session_id: "durable-session".to_owned(),
                round: round(),
                context_package_sha256: Some("a".repeat(64)),
            })
            .expect("start session");
        first_validation = service
            .submit("durable-session", submission("durable-message", "viewer", 42, private_text))
            .expect("submit before restart");
        assert_eq!(service.session("durable-session").unwrap().latest_event_sequence, 2);
    }

    {
        let mut service = SemanticEngineService::open(&database).expect("reopen service");
        let snapshot = service.session("durable-session").expect("restored session");
        assert_eq!(snapshot.state, SessionState::Active);
        assert_eq!(snapshot.latest_event_sequence, 2);
        let resumable = service.latest_active_session().expect("resumable session");
        assert_eq!(resumable.snapshot, snapshot);
        assert_eq!(resumable.round, round());
        assert_eq!(resumable.next_source_sequence, 43);
        let duplicate = service
            .submit("durable-session", submission("durable-message", "viewer", 42, private_text))
            .expect("idempotent retry after restart");
        assert_eq!(duplicate, first_validation);
        assert_eq!(service.session("durable-session").unwrap().latest_event_sequence, 2);
        assert_eq!(
            service.submit(
                "durable-session",
                submission("durable-message", "viewer", 42, "different content"),
            ),
            Err(ServiceError::IdentityConflict)
        );
        let resolution = service
            .resolve_session(
                "durable-session",
                OperatorResolutionRequest {
                    round_id: "round-1".to_owned(),
                    message_id: "durable-message".to_owned(),
                    verdict: ResolutionVerdict::Accepted,
                    target_id: Some("elden-ring".to_owned()),
                    note: "Revue après reprise".to_owned(),
                },
            )
            .expect("resolve restored validation");
        assert_eq!(resolution.final_decision, Decision::Accepted);
        assert_eq!(service.session("durable-session").unwrap().latest_event_sequence, 3);
    }

    {
        let mut service = SemanticEngineService::open(&database).expect("second reopen");
        let events = service.session_events("durable-session", 0, 10).expect("restored events");
        assert_eq!(events.events.len(), 3);
        assert_eq!(events.latest_sequence, 3);
        service
            .resolve_session(
                "durable-session",
                OperatorResolutionRequest {
                    round_id: "round-1".to_owned(),
                    message_id: "durable-message".to_owned(),
                    verdict: ResolutionVerdict::Accepted,
                    target_id: Some("elden-ring".to_owned()),
                    note: "Revue après reprise".to_owned(),
                },
            )
            .expect("idempotent resolution after restart");
        assert_eq!(service.session("durable-session").unwrap().latest_event_sequence, 3);
        service.end_session("durable-session").expect("end restored session");
    }

    let service = SemanticEngineService::open(&database).expect("reopen ended session");
    assert_eq!(service.session("durable-session").unwrap().state, SessionState::Ended);
    let database_bytes = std::fs::read(database).expect("read database");
    assert!(
        !database_bytes.windows(private_text.len()).any(|window| window == private_text.as_bytes())
    );
}
