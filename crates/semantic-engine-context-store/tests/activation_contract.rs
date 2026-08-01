use std::{fs, path::Path};

use rusqlite::params;
use semantic_engine_context_store::ContextStore;
use semantic_engine_package::import_package;

#[test]
fn operator_can_activate_a_new_context_and_rollback_after_restart() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let database = workspace.path().join("contexts.sqlite3");
    let first_descriptor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let second_descriptor = copy_package_with_version(
        &first_descriptor,
        &workspace.path().join("starter-v0.2.0"),
        "0.2.0",
    );

    let first = import_package(&first_descriptor).expect("first package must be inspectable");
    let second = import_package(&second_descriptor).expect("second package must be inspectable");
    let mut store = ContextStore::open(&database).expect("store must open");

    assert_eq!(store.current().expect("current context must load"), None);

    let activated_first = store.activate(&first).expect("first activation must succeed");
    assert_eq!(activated_first.version, "0.1.0");
    assert_eq!(store.current().expect("first context must load"), Some(activated_first));

    let activated_second = store.activate(&second).expect("second activation must succeed");
    assert_eq!(activated_second.version, "0.2.0");
    assert_eq!(store.current().expect("second context must load"), Some(activated_second));

    let restored =
        store.rollback().expect("rollback must be atomic").expect("a previous context must exist");
    assert_eq!(restored.version, "0.1.0");
    drop(store);

    let reopened = ContextStore::open(&database).expect("store must reopen");
    assert_eq!(reopened.current().expect("restored context must persist"), Some(restored));
}

#[test]
fn reactivating_the_current_context_does_not_create_a_rollback_point() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let descriptor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let imported = import_package(descriptor).expect("package must be inspectable");
    let mut store =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must open");

    store.activate(&imported).expect("first activation must succeed");
    store.activate(&imported).expect("repeated activation must be idempotent");

    assert_eq!(store.rollback().expect("rollback lookup must succeed"), None);
}

#[test]
fn a_published_package_version_cannot_be_replaced_with_different_bytes() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let first_descriptor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let conflicting_descriptor = copy_package_with_version(
        &first_descriptor,
        &workspace.path().join("conflicting-v0.1.0"),
        "0.1.0",
    );
    let conflicting_json = fs::read_to_string(&conflicting_descriptor)
        .expect("conflicting descriptor must load")
        .replacen(
            "\"name\": \"semantic-engine-starter-titles\"",
            "\"name\": \"different-bytes-same-version\"",
            1,
        );
    fs::write(&conflicting_descriptor, conflicting_json)
        .expect("conflicting descriptor must write");

    let first = import_package(first_descriptor).expect("first package must be inspectable");
    let conflicting = import_package(conflicting_descriptor)
        .expect("conflicting package remains structurally valid");
    let mut store =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must open");
    store.activate(&first).expect("first activation must succeed");

    let error = store
        .activate(&conflicting)
        .expect_err("same id and version with different bytes must be rejected");
    assert!(error.to_string().contains("version is immutable"));
}

#[test]
fn reactivating_a_legacy_stored_context_upgrades_derived_metadata_without_conflict() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let database = workspace.path().join("contexts.sqlite3");
    let first_descriptor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let second_descriptor = copy_package_with_version(
        &first_descriptor,
        &workspace.path().join("starter-v0.2.0"),
        "0.2.0",
    );
    let first = import_package(&first_descriptor).expect("first package must be inspectable");
    let second = import_package(&second_descriptor).expect("second package must be inspectable");

    let mut store = ContextStore::open(&database).expect("store must open");
    store.activate(&first).expect("first activation must succeed");
    store.activate(&second).expect("second activation must succeed");
    drop(store);

    let connection = rusqlite::Connection::open(&database).expect("legacy database must open");
    let payload: String = connection
        .query_row(
            "SELECT payload_json FROM context_versions WHERE package_sha256 = ?1",
            [&first.package_sha256],
            |row| row.get(0),
        )
        .expect("stored context payload must exist");
    let mut legacy: serde_json::Value =
        serde_json::from_str(&payload).expect("stored context payload must be JSON");
    let object = legacy.as_object_mut().expect("stored context payload must be an object");
    object.remove("licenses");
    object.remove("target_kinds");
    object.remove("metadata");
    object.remove("storage_format_version");
    object.remove("attachments");
    object.remove("targets_resource_metadata");
    connection
        .execute(
            "UPDATE context_versions SET payload_json = ?1 WHERE package_sha256 = ?2",
            params![serde_json::to_string(&legacy).unwrap(), first.package_sha256],
        )
        .expect("legacy payload must be installed");
    drop(connection);

    let mut reopened = ContextStore::open(&database).expect("legacy store must reopen");
    reopened.activate(&first).expect("same immutable bytes must upgrade stored metadata");
    let draft = reopened
        .exportable_draft(&first.package_sha256)
        .expect("upgraded context must export without losing metadata");
    assert_eq!(draft.licenses, first.licenses);
    assert_eq!(draft.target_kinds, first.target_kinds);
    assert_eq!(draft.metadata, first.metadata);
}

fn copy_package_with_version(
    source: &Path,
    destination: &Path,
    version: &str,
) -> std::path::PathBuf {
    fs::create_dir_all(destination.join("data")).expect("package data directory must exist");
    fs::copy(
        source.parent().expect("source package has a parent").join("data/titles.json"),
        destination.join("data/titles.json"),
    )
    .expect("titles fixture must copy");

    let descriptor = fs::read_to_string(source).expect("descriptor fixture must load");
    let descriptor =
        descriptor.replacen("\"version\": \"0.1.0\"", &format!("\"version\": \"{version}\""), 1);
    let destination_descriptor = destination.join("datapackage.json");
    fs::write(&destination_descriptor, descriptor).expect("descriptor fixture must write");
    destination_descriptor
}
