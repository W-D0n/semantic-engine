use semantic_engine_core::{Round, Submission, Validation, Validator};
use semantic_engine_package::{SourceMetadata, import_package};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContextPackagePreview {
    pub name: String,
    pub id: String,
    pub version: String,
    pub license: String,
    pub locales: Vec<String>,
    pub sources: Vec<SourceMetadata>,
    pub target_count: usize,
    pub package_sha256: String,
    pub targets_sha256: String,
}

#[tauri::command]
fn validate(round: Round, submission: Submission) -> Validation {
    Validator::default().validate(&round, &submission)
}

pub fn inspect_context_package(path: String) -> Result<ContextPackagePreview, String> {
    let imported = import_package(path).map_err(|error| error.to_string())?;
    Ok(ContextPackagePreview {
        name: imported.name,
        id: imported.id,
        version: imported.version.to_string(),
        license: imported.spdx_license_expression,
        locales: imported.locales,
        sources: imported.sources,
        target_count: imported.targets.len(),
        package_sha256: imported.package_sha256,
        targets_sha256: imported.targets_sha256,
    })
}

// Keep the Tauri macro on a private wrapper: exporting the annotated function
// duplicates generated command symbols when integration tests import the library.
#[tauri::command]
fn inspect_context_package_ipc(path: String) -> Result<ContextPackagePreview, String> {
    inspect_context_package(path)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![validate, inspect_context_package_ipc])
        .run(tauri::generate_context!())
        .expect("error while running Semantic Engine");
}
