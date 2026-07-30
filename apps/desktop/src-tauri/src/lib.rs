use std::{fs, sync::Mutex};

use semantic_engine_context_store::{ContextStore, StoredContext};
use semantic_engine_core::{Round, Submission, Validation, Validator};
use semantic_engine_package::{ImportedContext, SourceMetadata, import_package};
use serde::Serialize;
use tauri::{Manager, State};

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

impl From<&ImportedContext> for ContextPackagePreview {
    fn from(context: &ImportedContext) -> Self {
        Self {
            name: context.name.clone(),
            id: context.id.clone(),
            version: context.version.to_string(),
            license: context.spdx_license_expression.clone(),
            locales: context.locales.clone(),
            sources: context.sources.clone(),
            target_count: context.targets.len(),
            package_sha256: context.package_sha256.clone(),
            targets_sha256: context.targets_sha256.clone(),
        }
    }
}

impl From<&StoredContext> for ContextPackagePreview {
    fn from(context: &StoredContext) -> Self {
        Self {
            name: context.name.clone(),
            id: context.package_id.clone(),
            version: context.version.clone(),
            license: context.license.clone(),
            locales: context.locales.clone(),
            sources: context.sources.clone(),
            target_count: context.target_count,
            package_sha256: context.package_sha256.clone(),
            targets_sha256: context.targets_sha256.clone(),
        }
    }
}

#[tauri::command]
fn validate(round: Round, submission: Submission) -> Validation {
    Validator::default().validate(&round, &submission)
}

pub fn inspect_context_package(path: String) -> Result<ContextPackagePreview, String> {
    let imported = import_package(path).map_err(|error| error.to_string())?;
    Ok(ContextPackagePreview::from(&imported))
}

pub fn activate_context_package(
    path: String,
    expected_package_sha256: String,
    store: &mut ContextStore,
) -> Result<ContextPackagePreview, String> {
    let imported = import_package(path).map_err(|error| error.to_string())?;
    if imported.package_sha256 != expected_package_sha256 {
        return Err("context package changed after inspection; inspect it again before activation"
            .to_string());
    }
    let active = store.activate(&imported).map_err(|error| error.to_string())?;
    Ok(ContextPackagePreview::from(&active))
}

// Keep the Tauri macro on a private wrapper: exporting the annotated function
// duplicates generated command symbols when integration tests import the library.
#[tauri::command]
fn inspect_context_package_ipc(path: String) -> Result<ContextPackagePreview, String> {
    inspect_context_package(path)
}

#[tauri::command]
fn activate_context_package_ipc(
    path: String,
    expected_package_sha256: String,
    store: State<'_, Mutex<ContextStore>>,
) -> Result<ContextPackagePreview, String> {
    let mut store = store.lock().map_err(|_| "context store lock is poisoned".to_string())?;
    activate_context_package(path, expected_package_sha256, &mut store)
}

#[tauri::command]
fn current_context_ipc(
    store: State<'_, Mutex<ContextStore>>,
) -> Result<Option<ContextPackagePreview>, String> {
    let current = store
        .lock()
        .map_err(|_| "context store lock is poisoned".to_string())?
        .current()
        .map_err(|error| error.to_string())?;
    Ok(current.as_ref().map(ContextPackagePreview::from))
}

#[tauri::command]
fn rollback_context_ipc(
    store: State<'_, Mutex<ContextStore>>,
) -> Result<Option<ContextPackagePreview>, String> {
    let restored = store
        .lock()
        .map_err(|_| "context store lock is poisoned".to_string())?
        .rollback()
        .map_err(|error| error.to_string())?;
    Ok(restored.as_ref().map(ContextPackagePreview::from))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_directory = app.path().app_local_data_dir()?;
            fs::create_dir_all(&data_directory)?;
            let store = ContextStore::open(data_directory.join("contexts.sqlite3"))?;
            app.manage(Mutex::new(store));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            validate,
            inspect_context_package_ipc,
            activate_context_package_ipc,
            current_context_ipc,
            rollback_context_ipc
        ])
        .run(tauri::generate_context!())
        .expect("error while running Semantic Engine");
}
