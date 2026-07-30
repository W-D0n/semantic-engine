use std::{fs, path::PathBuf};

use semantic_engine_context_store::ContextStore;
use semantic_engine_desktop_lib::{activate_context_package, inspect_context_package};

#[test]
fn operator_can_inspect_a_valid_context_without_activating_it() {
    let descriptor = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packages/starter-titles/datapackage.json");

    let preview = inspect_context_package(descriptor.to_string_lossy().into_owned())
        .expect("starter package must be inspectable");

    assert_eq!(preview.name, "semantic-engine-starter-titles");
    assert_eq!(preview.version, "0.1.0");
    assert_eq!(preview.license, "CC0-1.0");
    assert_eq!(preview.locales, ["en", "fr"]);
    assert_eq!(preview.target_count, 84);
    assert_eq!(
        preview.package_sha256,
        "b1ccb8cc500d04011e66ccfd09c52711be4f190d58e1c1a25cf7363106eb432e"
    );
    assert_eq!(preview.sources.len(), 1);
}

#[test]
fn activation_rejects_a_package_replaced_after_inspection() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packages/starter-titles");
    let package = workspace.path().join("package");
    fs::create_dir_all(package.join("data")).expect("package directory must be writable");
    fs::copy(source.join("datapackage.json"), package.join("datapackage.json"))
        .expect("descriptor must copy");
    fs::copy(source.join("data/titles.json"), package.join("data/titles.json"))
        .expect("resource must copy");
    let descriptor = package.join("datapackage.json");

    let preview = inspect_context_package(descriptor.to_string_lossy().into_owned())
        .expect("copied package must be inspectable");
    let replaced = fs::read_to_string(&descriptor).expect("descriptor must load").replacen(
        "\"name\": \"semantic-engine-starter-titles\"",
        "\"name\": \"semantic-engine-replaced-titles\"",
        1,
    );
    fs::write(&descriptor, replaced).expect("descriptor replacement must write");
    let mut store =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must open");

    let error = activate_context_package(
        descriptor.to_string_lossy().into_owned(),
        preview.package_sha256,
        &mut store,
    )
    .expect_err("activation must reject bytes different from the preview");

    assert!(error.contains("changed after inspection"));
    assert_eq!(store.current().expect("store must remain readable"), None);
}
