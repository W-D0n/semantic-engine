use std::path::PathBuf;

use semantic_engine_desktop_lib::inspect_context_package;

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
