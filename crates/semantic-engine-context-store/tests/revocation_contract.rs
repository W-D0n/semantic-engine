use std::path::Path;

use semantic_engine_context_store::{ChannelPackageStatus, ContextStore};
use semantic_engine_package::import_package;

fn starter() -> semantic_engine_package::ImportedContext {
    import_package(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/starter-titles/datapackage.json"),
    )
    .expect("starter package must import")
}

fn status(revocation_reason: Option<&str>) -> ChannelPackageStatus {
    let imported = starter();
    ChannelPackageStatus {
        channel_root_sha256: "a".repeat(64),
        archive_sha256: "b".repeat(64),
        package_id: imported.id,
        package_version: imported.version.to_string(),
        revocation_reason: revocation_reason.map(str::to_owned),
    }
}

#[test]
fn a_trusted_channel_revocation_blocks_future_activation() {
    let workspace = tempfile::tempdir().unwrap();
    let mut store = ContextStore::open(workspace.path().join("contexts.sqlite3")).unwrap();
    store.apply_channel_statuses(&[status(Some("invalid-data"))]).unwrap();

    let error = store.activate(&starter()).expect_err("revoked identity must stay blocked");

    assert!(error.to_string().contains("revoked by a trusted channel"));
    assert_eq!(store.current().unwrap(), None);
}

#[test]
fn refreshing_a_revocation_quarantines_the_matching_active_context() {
    let workspace = tempfile::tempdir().unwrap();
    let database = workspace.path().join("contexts.sqlite3");
    let mut store = ContextStore::open(&database).unwrap();
    store.activate(&starter()).unwrap();

    let quarantined = store.apply_channel_statuses(&[status(Some("withdrawn"))]).unwrap();

    assert_eq!(
        quarantined.as_ref().map(|item| item.package_id.as_str()),
        Some(starter().id.as_str())
    );
    assert_eq!(store.current().unwrap(), None);
    drop(store);
    let mut reopened = ContextStore::open(database).unwrap();
    assert!(reopened.activate(&starter()).is_err());
}

#[test]
fn a_later_clean_refresh_cannot_erase_a_revocation() {
    let workspace = tempfile::tempdir().unwrap();
    let mut store = ContextStore::open(workspace.path().join("contexts.sqlite3")).unwrap();
    store.apply_channel_statuses(&[status(Some("legal"))]).unwrap();
    store.apply_channel_statuses(&[status(None)]).unwrap();

    assert!(store.activate(&starter()).is_err());
}
