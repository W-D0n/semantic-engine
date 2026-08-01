use std::fs;

use semantic_engine_audit_store::{AuditError, AuditStore, RetentionPolicy};
use semantic_engine_core::{
    AnswerTarget, Decision, Evidence, EvidenceKind, OperatorResolution, Round, Submission,
    ValidationPolicy, Validator,
};
use tempfile::tempdir;

fn validation(
    message_id: &str,
    participant_id: &str,
    source_sequence: u64,
    text: &str,
) -> semantic_engine_core::Validation {
    Validator::default().validate(
        &Round {
            id: "movie-round".to_owned(),
            targets: vec![AnswerTarget {
                id: "alien".to_owned(),
                canonical: "Alien".to_owned(),
                aliases: vec![],
            }],
            policy: ValidationPolicy::default(),
        },
        &Submission {
            message_id: message_id.to_owned(),
            participant_id: participant_id.to_owned(),
            source_sequence,
            text: text.to_owned(),
        },
    )
}

#[test]
fn audit_is_idempotent_and_never_persists_raw_chat_text() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("audit.sqlite3");
    let mut store = AuditStore::open(&database, RetentionPolicy::default()).expect("audit store");
    let mut result = validation("message-1", "viewer-1", 1, "Alien");
    result.evidence = vec![Evidence {
        kind: EvidenceKind::AmbiguousExpression,
        matched_expression: "do-not-persist-this-chat-text".to_owned(),
    }];
    let context_sha = "a".repeat(64);

    let first = store.record_validation(&result, Some(&context_sha)).expect("first record");
    let duplicate = store.record_validation(&result, Some(&context_sha)).expect("idempotent retry");
    assert_eq!(first, duplicate);
    assert_eq!(first.schema_version, 1);
    assert_eq!(first.validation.evidence_kinds.len(), result.evidence.len());
    drop(store);

    let bytes = fs::read(database).expect("database bytes");
    assert!(!String::from_utf8_lossy(&bytes).contains("do-not-persist-this-chat-text"));
}

#[test]
fn conflicting_retry_is_rejected_and_resolution_requires_its_validation() {
    let mut store = AuditStore::open_in_memory(RetentionPolicy::default()).expect("audit store");
    let accepted = validation("message-1", "viewer-1", 1, "Alien");
    store.record_validation(&accepted, None).expect("record validation");

    let mut conflicting = accepted.clone();
    conflicting.participant_id = "fabricated-viewer".to_owned();
    assert_eq!(store.record_validation(&conflicting, None), Err(AuditError::Conflict));

    let missing = OperatorResolution {
        round_id: "movie-round".to_owned(),
        message_id: "missing".to_owned(),
        participant_id: "viewer-1".to_owned(),
        source_sequence: 2,
        original_decision: Decision::Abstained,
        final_decision: Decision::Rejected,
        target_id: None,
        note: String::new(),
    };
    assert_eq!(store.record_resolution(&missing), Err(AuditError::MissingValidation));
}

#[test]
fn source_order_resolution_retention_and_deletion_are_explicit() {
    let mut store =
        AuditStore::open_in_memory(RetentionPolicy { max_validations: 2, max_age_seconds: None })
            .expect("audit store");

    let late = validation("message-3", "viewer-3", 30, "Alien");
    let first = validation("message-1", "viewer-1", 10, "Alien");
    let middle = validation("message-2", "viewer-2", 20, "Alien");
    store.record_validation(&late, None).expect("late arrival");
    store.record_validation(&first, None).expect("first source event");
    store.record_validation(&middle, None).expect("middle source event");

    let entries = store.list_round("movie-round").expect("round audit");
    assert_eq!(
        entries.iter().map(|entry| entry.validation.source_sequence).collect::<Vec<_>>(),
        vec![10, 20]
    );

    let resolution = OperatorResolution {
        round_id: "movie-round".to_owned(),
        message_id: "message-2".to_owned(),
        participant_id: "viewer-2".to_owned(),
        source_sequence: 20,
        original_decision: middle.decision,
        final_decision: Decision::Rejected,
        target_id: None,
        note: "operator correction".to_owned(),
    };
    let resolved = store.record_resolution(&resolution).expect("linked resolution");
    assert_eq!(
        resolved.resolution.as_ref().expect("resolution").final_decision,
        Decision::Rejected
    );
    assert_eq!(store.record_resolution(&resolution).expect("idempotent resolution"), resolved);

    assert_eq!(store.delete_round("movie-round").expect("delete round"), 2);
    assert!(store.recent(10).expect("recent audit").is_empty());
}

#[test]
fn purge_removes_identifier_payload_from_the_database_file() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("audit.sqlite3");
    let mut store = AuditStore::open(&database, RetentionPolicy::default()).expect("audit store");
    let result = validation("erase-message", "erase-this-participant", 1, "Alien");
    store.record_validation(&result, None).expect("record validation");
    assert_eq!(store.purge_all().expect("purge audit"), 1);
    drop(store);

    let bytes = fs::read(database).expect("database bytes");
    assert!(!String::from_utf8_lossy(&bytes).contains("erase-this-participant"));
}
