use std::{fs, path::Path};

use semantic_engine_context_store::ContextStore;
use semantic_engine_core::AnswerTarget;
use semantic_engine_package::{export_package, import_package};

#[test]
fn local_drafts_export_as_a_new_immutable_package_that_reimports_cleanly() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let descriptor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let imported = import_package(descriptor).expect("starter package must import");
    let original_package_sha256 = imported.package_sha256.clone();
    let mut store =
        ContextStore::open(workspace.path().join("contexts.sqlite3")).expect("store must open");
    store.activate(&imported).expect("starter package must activate");
    store
        .save_target_draft(
            &original_package_sha256,
            AnswerTarget {
                id: "witcher-3".to_owned(),
                canonical: "The Witcher 3: Wild Hunt".to_owned(),
                aliases: vec!["Witcher 3".to_owned(), "TW3".to_owned()],
            },
        )
        .expect("draft must persist");

    let draft =
        store.exportable_draft(&original_package_sha256).expect("active draft must be exportable");
    let exported_descriptor = workspace.path().join("starter-titles-0.2.0/datapackage.json");
    let exported = export_package(&draft, "0.2.0", &exported_descriptor)
        .expect("a higher immutable version must export");
    let reimported = import_package(&exported_descriptor).expect("export must pass normal import");

    assert_eq!(exported, reimported);
    assert_eq!(reimported.id, imported.id);
    assert_eq!(reimported.version.to_string(), "0.2.0");
    assert_eq!(reimported.spdx_license_expression, imported.spdx_license_expression);
    assert_eq!(reimported.licenses, imported.licenses);
    assert_eq!(reimported.metadata, imported.metadata);
    assert_eq!(reimported.targets_resource_metadata, imported.targets_resource_metadata);
    assert_eq!(
        reimported.target_kinds.get("witcher-3"),
        Some(&semantic_engine_package::ContextTargetKind::Game)
    );
    assert_ne!(reimported.package_sha256, original_package_sha256);
    let tuned = reimported
        .targets
        .iter()
        .find(|target| target.id == "witcher-3")
        .expect("tuned target must remain present");
    assert_eq!(tuned.aliases, ["Witcher 3", "TW3"]);
    assert!(
        exported_descriptor.parent().unwrap().join("profile/context-package.schema.json").is_file()
    );
    assert!(
        exported_descriptor.parent().unwrap().join("profile/title-resource.schema.json").is_file()
    );
    let export_root = exported_descriptor.parent().unwrap();
    for support_file in
        ["README.md", "LICENSE.md", "SHA256SUMS.txt", "profile/vendor/datapackage-v2.schema.json"]
    {
        assert!(export_root.join(support_file).is_file(), "missing {support_file}");
    }
    let exported_profile =
        std::fs::read_to_string(export_root.join("profile/context-package.schema.json"))
            .expect("exported profile must be readable");
    assert!(exported_profile.contains("vendor/datapackage-v2.schema.json"));
    assert!(!exported_profile.contains("https://datapackage.org/profiles/2.0"));
    let descriptor_json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&exported_descriptor).expect("exported descriptor must be readable"),
    )
    .expect("exported descriptor must remain JSON");
    assert!(descriptor_json["sources"][0].get("path").is_none());
    assert!(descriptor_json["sources"][0].get("version").is_some());
    assert_eq!(descriptor_json["licenses"][0]["title"], "Creative Commons Zero v1.0 Universal");
    assert_eq!(descriptor_json["resources"][0]["format"], "json");

    let published_bytes = std::fs::read(&exported_descriptor).expect("export must be readable");
    let overwrite = export_package(&draft, "0.3.0", &exported_descriptor)
        .expect_err("an existing exported version must never be overwritten");
    assert!(overwrite.to_string().contains("destination already exists"));
    assert_eq!(
        std::fs::read(&exported_descriptor).expect("published export must remain readable"),
        published_bytes
    );

    let invalid_descriptor = workspace.path().join("starter-titles-0.1.0/datapackage.json");
    let downgrade = export_package(&draft, "0.1.0", &invalid_descriptor)
        .expect_err("an export version must be greater than its active base");
    assert!(downgrade.to_string().contains("greater than its base version"));
    assert!(!invalid_descriptor.parent().unwrap().exists());
}

#[test]
fn local_license_and_provenance_files_survive_export_with_their_relative_paths() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let starter = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/starter-titles/datapackage.json");
    let source_root = workspace.path().join("source-package");
    fs::create_dir_all(source_root.join("data")).expect("source data directory must exist");
    fs::copy(
        starter.parent().unwrap().join("data/titles.json"),
        source_root.join("data/titles.json"),
    )
    .expect("titles must copy");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&fs::read(&starter).expect("starter descriptor must be readable"))
            .expect("starter descriptor must be JSON");
    descriptor["licenses"][0]["path"] = "legal/license.pdf".into();
    descriptor["sources"][0]["path"] = "provenance/SOURCE.md".into();
    fs::create_dir_all(source_root.join("legal")).unwrap();
    fs::create_dir_all(source_root.join("provenance")).unwrap();
    let binary_license = b"%PDF-1.7\n\xff\0binary-license";
    fs::write(source_root.join("legal/license.pdf"), binary_license).unwrap();
    fs::write(source_root.join("provenance/SOURCE.md"), "original provenance\n").unwrap();
    let source_descriptor = source_root.join("datapackage.json");
    fs::write(&source_descriptor, serde_json::to_vec_pretty(&descriptor).unwrap()).unwrap();

    let imported = import_package(&source_descriptor).expect("local metadata files must import");
    let mut store = ContextStore::open(workspace.path().join("contexts.sqlite3")).unwrap();
    store.activate(&imported).unwrap();
    let draft = store.exportable_draft(&imported.package_sha256).unwrap();
    let exported_descriptor = workspace.path().join("exported-0.2.0/datapackage.json");
    export_package(&draft, "0.2.0", &exported_descriptor)
        .expect("local metadata files must export");
    let export_root = exported_descriptor.parent().unwrap();

    assert_eq!(fs::read(export_root.join("legal/license.pdf")).unwrap(), binary_license);
    assert_eq!(
        fs::read_to_string(export_root.join("provenance/SOURCE.md")).unwrap(),
        "original provenance\n"
    );
    let exported: serde_json::Value =
        serde_json::from_slice(&fs::read(&exported_descriptor).unwrap()).unwrap();
    assert_eq!(exported["licenses"][0]["path"], "legal/license.pdf");
    assert_eq!(exported["sources"][0]["path"], "provenance/SOURCE.md");
    let checksums = fs::read_to_string(export_root.join("SHA256SUMS.txt")).unwrap();
    assert!(checksums.contains("legal/license.pdf"));
    assert!(checksums.contains("provenance/SOURCE.md"));
}
