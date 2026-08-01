use std::collections::BTreeMap;

use semantic_engine_source::{
    CreateSource, SourceDesiredState, SourceError, SourceMessage, SourceStore, UpdateSource,
};

fn twitch_source(source_id: &str) -> CreateSource {
    CreateSource {
        source_id: source_id.to_owned(),
        adapter: "twitch-eventsub".to_owned(),
        display_name: "Chat Twitch principal".to_owned(),
        settings: BTreeMap::from([
            ("broadcaster_login".to_owned(), "example_channel".to_owned()),
            ("client_id".to_owned(), "public-client-id".to_owned()),
        ]),
        credential_id: Some(format!("credential-{source_id}")),
    }
}

#[test]
fn source_lifecycle_is_durable_revisioned_and_safe_to_remove() {
    let workspace = tempfile::tempdir().unwrap();
    let database = workspace.path().join("sources.sqlite3");
    let mut store = SourceStore::open(&database).unwrap();

    let created = store.add(twitch_source("twitch-main")).unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(created.desired_state, SourceDesiredState::Paused);

    let active = store
        .set_desired_state("twitch-main", created.revision, SourceDesiredState::Active)
        .unwrap();
    assert_eq!(active.revision, 2);
    assert_eq!(store.remove("twitch-main", active.revision), Err(SourceError::MustBePaused));
    drop(store);

    let mut reopened = SourceStore::open(&database).unwrap();
    let restored = reopened.get("twitch-main").unwrap();
    assert_eq!(restored, active);
    assert_eq!(reopened.list().unwrap(), vec![active.clone()]);

    let paused = reopened
        .set_desired_state("twitch-main", active.revision, SourceDesiredState::Paused)
        .unwrap();
    reopened.remove("twitch-main", paused.revision).unwrap();
    assert!(reopened.list().unwrap().is_empty());
}

#[test]
fn edits_use_optimistic_revisions_and_do_not_replace_adapter_identity() {
    let mut store = SourceStore::open_in_memory().unwrap();
    let created = store.add(twitch_source("twitch-main")).unwrap();
    let updated = store
        .update(
            "twitch-main",
            UpdateSource {
                expected_revision: created.revision,
                display_name: "Chat Twitch FR".to_owned(),
                settings: BTreeMap::from([
                    ("broadcaster_login".to_owned(), "chaine_fr".to_owned()),
                    ("client_id".to_owned(), "public-client-id".to_owned()),
                ]),
                credential_id: created.definition.credential_id.clone(),
            },
        )
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.definition.adapter, "twitch-eventsub");
    assert_eq!(
        store.update(
            "twitch-main",
            UpdateSource {
                expected_revision: created.revision,
                display_name: "stale".to_owned(),
                settings: BTreeMap::new(),
                credential_id: None,
            },
        ),
        Err(SourceError::Conflict)
    );
}

#[test]
fn plaintext_secrets_and_unbounded_configuration_are_rejected() {
    let mut store = SourceStore::open_in_memory().unwrap();
    let mut secret = twitch_source("twitch-secret");
    secret.settings.insert("access_token".to_owned(), "do-not-store-this".to_owned());
    assert!(matches!(store.add(secret), Err(SourceError::Invalid(_))));

    let mut oversized = twitch_source("twitch-large");
    oversized.settings.insert("channel".to_owned(), "x".repeat(513));
    assert!(matches!(store.add(oversized), Err(SourceError::Invalid(_))));
}

#[test]
fn source_messages_translate_without_leaking_adapter_types_into_the_engine() {
    let message = SourceMessage {
        source_id: "twitch-main".to_owned(),
        message_id: "platform-message-1".to_owned(),
        participant_id: "viewer-7".to_owned(),
        source_sequence: 42,
        text: "eldern ring".to_owned(),
        occurred_at_ms: 123,
    };
    let submission = message.into_submission();
    assert_eq!(submission.message_id, "platform-message-1");
    assert_eq!(submission.participant_id, "viewer-7");
    assert_eq!(submission.source_sequence, 42);
    assert_eq!(submission.text, "eldern ring");
}
