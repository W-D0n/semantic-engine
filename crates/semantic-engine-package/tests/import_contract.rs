use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use semantic_engine_package::{PackageError, import_package};

#[test]
fn starter_context_package_is_importable_and_integrity_checked() {
    let descriptor = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");

    let imported = import_package(descriptor).expect("starter package must import");

    assert_eq!(imported.name, "semantic-engine-starter-titles");
    assert_eq!(imported.version.to_string(), "0.1.0");
    assert_eq!(imported.locales, ["en", "fr"]);
    assert_eq!(imported.spdx_license_expression, "CC0-1.0");
    assert_eq!(imported.sources[0].title, "Manually curated Semantic Engine test corpus");
    assert_eq!(imported.targets.len(), 84);
    assert_eq!(
        imported.targets_sha256,
        "062cc8a6223685ac8fb0d6112b8393a5d849dd8d4dcba648dac719679a82b8c1"
    );
    assert_eq!(
        imported.package_sha256,
        "b1ccb8cc500d04011e66ccfd09c52711be4f190d58e1c1a25cf7363106eb432e"
    );
    assert!(imported.targets.iter().any(|target| target.id == "spirited-away"
        && target.aliases.iter().any(|alias| alias == "Le Voyage de Chihiro")));
}

#[test]
fn a_same_size_tampered_resource_is_rejected_by_sha256() {
    let source_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/starter-titles");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after the Unix epoch")
        .as_nanos();
    let temporary_root = std::env::temp_dir()
        .join(format!("semantic-engine-package-test-{}-{unique}", std::process::id()));
    let data_root = temporary_root.join("data");
    fs::create_dir_all(&data_root).expect("temporary package directory must be writable");
    fs::copy(source_root.join("datapackage.json"), temporary_root.join("datapackage.json"))
        .expect("descriptor must be copied");

    let original = fs::read_to_string(source_root.join("data/titles.json"))
        .expect("starter titles must be readable");
    let tampered = original.replacen("Elden Ring", "Elden Rink", 1);
    assert_eq!(original.len(), tampered.len());
    fs::write(data_root.join("titles.json"), tampered).expect("tampered resource must be written");

    let result = import_package(temporary_root.join("datapackage.json"));
    assert!(matches!(result, Err(PackageError::Integrity { .. })));

    fs::remove_dir_all(&temporary_root).expect("temporary package must be removed");
}
