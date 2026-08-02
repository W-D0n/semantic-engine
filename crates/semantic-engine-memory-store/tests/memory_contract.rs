use std::time::Duration;

use semantic_engine_memory_store::{MemoryPolicy, MemoryState, RecognitionMemoryStore};

const CONTEXT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn store(capacity: usize, ttl_seconds: u64) -> RecognitionMemoryStore {
    RecognitionMemoryStore::open_in_memory(MemoryPolicy {
        capacity,
        ttl: Duration::from_secs(ttl_seconds),
    })
    .expect("memory store")
}

#[test]
fn confirmed_expression_is_isolated_by_context_and_target() {
    let mut memory = store(8, 60);
    let entry = memory
        .remember(CONTEXT, "portal-2", "Portail deux", &[7; 32], 1_000)
        .expect("remember expression");

    let matches = memory
        .lookup(CONTEXT, "portail DEUX", &["portal-2".into(), "portal".into()], 1_500)
        .expect("lookup");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, entry.id);
    assert_eq!(matches[0].target_id, "portal-2");
    assert_eq!(matches[0].use_count, 0);
    memory.mark_used(CONTEXT, std::slice::from_ref(&entry.id), 1_500).expect("mark used");
    assert_eq!(memory.list_active(CONTEXT, 8, 1_500).unwrap()[0].use_count, 1);
    assert!(
        memory
            .lookup(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "portail deux",
                &["portal-2".into()],
                1_600,
            )
            .expect("other context lookup")
            .is_empty()
    );
}

#[test]
fn revocation_and_expiration_make_learning_reversible() {
    let mut memory = store(8, 1);
    let revoked = memory
        .remember(CONTEXT, "portal", "blue door", &[1; 32], 1_000)
        .expect("remember revoked fixture");
    assert_eq!(
        memory.revoke(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &revoked.id,
            1_050,
        ),
        Err(semantic_engine_memory_store::MemoryError::Missing)
    );
    memory.revoke(CONTEXT, &revoked.id, 1_100).expect("revoke");
    assert!(
        memory
            .lookup(CONTEXT, "blue door", &["portal".into()], 1_200)
            .expect("revoked lookup")
            .is_empty()
    );

    let expiring = memory
        .remember(CONTEXT, "portal-2", "orange door", &[2; 32], 2_000)
        .expect("remember expiring fixture");
    assert!(
        memory
            .lookup(CONTEXT, "orange door", &["portal-2".into()], 3_000)
            .expect("expired lookup")
            .is_empty()
    );
    let listed = memory.list(CONTEXT, 8, 3_000).expect("list history");
    assert_eq!(
        listed.iter().find(|entry| entry.id == revoked.id).unwrap().state,
        MemoryState::Revoked
    );
    assert_eq!(
        listed.iter().find(|entry| entry.id == expiring.id).unwrap().state,
        MemoryState::Expired
    );
}

#[test]
fn capacity_evicts_the_least_recently_used_active_entry() {
    let mut memory = store(2, 60);
    let first = memory.remember(CONTEXT, "one", "first", &[1; 32], 1_000).expect("first");
    let second = memory.remember(CONTEXT, "two", "second", &[2; 32], 1_100).expect("second");
    memory.lookup(CONTEXT, "first", &["one".into()], 1_200).expect("find first");
    memory.mark_used(CONTEXT, std::slice::from_ref(&first.id), 1_200).expect("touch first");
    memory.remember(CONTEXT, "three", "third", &[3; 32], 1_300).expect("third");

    let listed = memory.list(CONTEXT, 8, 1_400).expect("list");
    assert_eq!(
        listed.iter().find(|entry| entry.id == first.id).unwrap().state,
        MemoryState::Active
    );
    assert_eq!(
        listed.iter().find(|entry| entry.id == second.id).unwrap().state,
        MemoryState::Evicted
    );
}
