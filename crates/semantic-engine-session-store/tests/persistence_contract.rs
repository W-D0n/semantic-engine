use semantic_engine_session_store::{
    SessionStore, StoredDelivery, StoredEvent, StoredSessionHeader, StoredSessionState,
};

fn header() -> StoredSessionHeader {
    StoredSessionHeader {
        session_id: "live-1".into(),
        definition_fingerprint: [7; 32],
        definition_json: r#"{"session_id":"live-1"}"#.into(),
        state: StoredSessionState::Active,
        created_at_ms: 10,
        ended_at_ms: None,
        latest_event_sequence: 1,
    }
}

fn event(sequence: u64) -> StoredEvent {
    StoredEvent {
        sequence,
        occurred_at_ms: 10 + sequence,
        payload_json: format!(r#"{{"sequence":{sequence}}}"#),
    }
}

fn delivery(sequence: u64) -> StoredDelivery {
    StoredDelivery {
        message_id: format!("message-{sequence}"),
        sequence,
        request_fingerprint: [sequence as u8; 32],
        validation_json: format!(r#"{{"message_id":"message-{sequence}"}}"#),
        resolution_emitted: false,
    }
}

#[test]
fn session_events_and_idempotency_state_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("state.sqlite3");
    {
        let mut store = SessionStore::open(&database).unwrap();
        store.create_session(&header(), &event(1)).unwrap();
        store.record_validation("live-1", &event(2), &delivery(2), 10, 10).unwrap();
        store.record_resolution("live-1", "message-2", &event(3), 10).unwrap();
    }
    let store = SessionStore::open(&database).unwrap();
    let sessions = store.load_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].events.len(), 3);
    assert_eq!(sessions[0].deliveries.len(), 1);
    assert!(sessions[0].deliveries[0].resolution_emitted);
    assert!(!sessions[0].events[1].payload_json.contains("chat text"));
}

#[test]
fn retention_is_bounded_without_reusing_event_sequences() {
    let mut store = SessionStore::open_in_memory().unwrap();
    store.create_session(&header(), &event(1)).unwrap();
    for sequence in 2..=5 {
        store.record_validation("live-1", &event(sequence), &delivery(sequence), 2, 2).unwrap();
    }
    let sessions = store.load_sessions().unwrap();
    assert_eq!(sessions[0].header.latest_event_sequence, 5);
    assert_eq!(sessions[0].events.iter().map(|item| item.sequence).collect::<Vec<_>>(), [4, 5]);
    assert_eq!(sessions[0].deliveries.iter().map(|item| item.sequence).collect::<Vec<_>>(), [4, 5]);
}

#[test]
fn ending_is_durable_and_rejects_later_writes() {
    let mut store = SessionStore::open_in_memory().unwrap();
    store.create_session(&header(), &event(1)).unwrap();
    store.end_session("live-1", &event(2), 10).unwrap();
    let error = store.record_validation("live-1", &event(3), &delivery(3), 10, 10).unwrap_err();
    assert_eq!(error.to_string(), "durable session has ended");
    let sessions = store.load_sessions().unwrap();
    assert_eq!(sessions[0].header.state, StoredSessionState::Ended);
    assert_eq!(sessions[0].header.ended_at_ms, Some(12));
}
