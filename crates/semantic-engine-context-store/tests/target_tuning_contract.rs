use std::{fs, path::Path};

use semantic_engine_context_store::ContextStore;
use semantic_engine_core::AnswerTarget;
use semantic_engine_package::import_package;

#[test]
fn operator_can_search_tune_and_restore_a_target_without_mutating_the_package() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let descriptor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let imported = import_package(descriptor).expect("starter package must import");
    let package_sha256 = imported.package_sha256.clone();
    let mut store =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must open");
    store.activate(&imported).expect("starter package must activate");

    let published =
        store.find_targets("witcher iii", 10).expect("active targets must be searchable");
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].id, "witcher-3");
    assert!(!published[0].is_draft);

    store
        .save_target_draft(
            &package_sha256,
            AnswerTarget {
                id: "witcher-3".to_owned(),
                canonical: "The Witcher 3: Wild Hunt".to_owned(),
                aliases: vec![
                    "Witcher 3".to_owned(),
                    "The Witcher III".to_owned(),
                    "TW3".to_owned(),
                ],
            },
        )
        .expect("a valid local draft must persist");
    drop(store);

    let mut reopened =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must reopen");
    let tuned =
        reopened.find_targets("tw3", 10).expect("draft aliases must be searchable after restart");
    assert_eq!(tuned.len(), 1);
    assert!(tuned[0].is_draft);
    assert_eq!(tuned[0].aliases.last().map(String::as_str), Some("TW3"));
    assert_eq!(tuned[0].package_sha256, package_sha256);

    reopened.discard_target_draft(&package_sha256, "witcher-3").expect("draft reset must succeed");
    assert!(reopened.find_targets("tw3", 10).expect("search must remain usable").is_empty());
    assert_eq!(
        reopened.find_targets("witcher iii", 10).expect("published aliases must be restored")[0]
            .canonical,
        "The Witcher 3: Wild Hunt"
    );
}

#[test]
fn local_draft_cannot_create_a_target_outside_the_active_package() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let descriptor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let imported = import_package(descriptor).expect("starter package must import");
    let package_sha256 = imported.package_sha256.clone();
    let mut store =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must open");
    store.activate(&imported).expect("starter package must activate");

    let result = store.save_target_draft(
        &package_sha256,
        AnswerTarget {
            id: "injected-target".to_owned(),
            canonical: "Untrusted title".to_owned(),
            aliases: vec![],
        },
    );

    assert!(result.is_err());
}

#[test]
fn draft_write_is_refused_when_the_active_context_changed_after_search() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let first_descriptor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let second_descriptor = copy_package_with_version(
        &first_descriptor,
        &workspace.path().join("starter-v0.2.0"),
        "0.2.0",
    );
    let first = import_package(&first_descriptor).expect("first package must import");
    let second = import_package(second_descriptor).expect("second package must import");
    let mut store =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must open");
    store.activate(&first).expect("first package must activate");
    let selected =
        store.find_targets("witcher iii", 1).expect("target must be selected")[0].clone();
    store
        .save_target_draft(
            &selected.package_sha256,
            AnswerTarget {
                id: selected.id.clone(),
                canonical: selected.canonical.clone(),
                aliases: selected.aliases.clone(),
            },
        )
        .expect("draft in first package must persist");

    store.activate(&second).expect("second package must activate");
    let save_error = store
        .save_target_draft(
            &selected.package_sha256,
            AnswerTarget {
                id: selected.id.clone(),
                canonical: "Stale edit".to_owned(),
                aliases: vec![],
            },
        )
        .expect_err("stale save must be rejected");
    let discard_error = store
        .discard_target_draft(&selected.package_sha256, &selected.id)
        .expect_err("stale discard must be rejected");

    assert!(save_error.to_string().contains("active context changed"));
    assert!(discard_error.to_string().contains("active context changed"));
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
