use std::path::Path;

use semantic_engine_context_store::ContextStore;
use semantic_engine_core::AnswerTarget;
use semantic_engine_desktop_lib::export_context_draft;
use semantic_engine_package::import_package;

#[test]
fn desktop_export_creates_a_named_version_directory_under_the_selected_parent() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let imported = import_package(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packages/starter-titles/datapackage.json"),
    )
    .expect("starter package must import");
    let mut store =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must open");
    store.activate(&imported).expect("context must activate");
    store
        .save_target_draft(
            &imported.package_sha256,
            AnswerTarget {
                id: "witcher-3".to_owned(),
                canonical: "The Witcher 3: Wild Hunt".to_owned(),
                aliases: vec!["TW3".to_owned()],
            },
        )
        .expect("draft must persist");

    let export = export_context_draft(
        workspace.path().to_string_lossy().into_owned(),
        imported.package_sha256,
        "0.2.0".to_owned(),
        &store,
    )
    .expect("desktop export must succeed");

    assert_eq!(export.preview.version, "0.2.0");
    let descriptor = Path::new(&export.descriptor_path);
    assert_eq!(descriptor.file_name().and_then(|name| name.to_str()), Some("datapackage.json"));
    assert_eq!(
        descriptor.parent().and_then(Path::file_name).and_then(|name| name.to_str()),
        Some("semantic-engine-starter-titles-0.2.0")
    );
    assert!(descriptor.is_file());
}
