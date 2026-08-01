use std::{collections::VecDeque, fs, path::PathBuf, sync::Mutex};

use semantic_engine_context_store::{ContextStore, StoredContext, TargetRecord};
use semantic_engine_core::{
    AnswerTarget, OperatorResolution, OperatorResolutionRequest, ResolutionIssue, Round,
    Submission, Validation, Validator, resolve_validation,
};
use semantic_engine_package::{ImportedContext, SourceMetadata, export_package, import_package};
use semver::Version;
use serde::Serialize;
use tauri::{Manager, State};

const MAX_RECORDED_VALIDATIONS: usize = 256;

#[derive(Clone)]
struct RecordedValidation {
    round: Round,
    validation: Validation,
}

#[derive(Default)]
struct ValidationLedger {
    entries: VecDeque<RecordedValidation>,
}

impl ValidationLedger {
    fn record(&mut self, round: Round, validation: Validation) {
        self.entries.retain(|entry| {
            entry.validation.round_id != validation.round_id
                || entry.validation.message_id != validation.message_id
        });
        self.entries.push_back(RecordedValidation { round, validation });
        while self.entries.len() > MAX_RECORDED_VALIDATIONS {
            self.entries.pop_front();
        }
    }
}

fn validate_and_record(
    round: Round,
    submission: Submission,
    ledger: &mut ValidationLedger,
) -> Validation {
    let validation = Validator::default().validate(&round, &submission);
    ledger.record(round, validation.clone());
    validation
}

fn resolve_recorded(
    request: OperatorResolutionRequest,
    ledger: &ValidationLedger,
) -> Result<OperatorResolution, ResolutionIssue> {
    let recorded = ledger
        .entries
        .iter()
        .find(|entry| {
            entry.validation.round_id == request.round_id
                && entry.validation.message_id == request.message_id
        })
        .ok_or(ResolutionIssue::ValidationMismatch)?;
    resolve_validation(&recorded.round, &recorded.validation, request)
}
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExportedContextPackage {
    pub preview: ContextPackagePreview,
    pub descriptor_path: String,
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
fn validate(
    round: Round,
    submission: Submission,
    ledger: State<'_, Mutex<ValidationLedger>>,
) -> Result<Validation, String> {
    let mut ledger = ledger.lock().map_err(|_| "validation ledger lock is poisoned".to_string())?;
    Ok(validate_and_record(round, submission, &mut ledger))
}

#[tauri::command]
fn resolve(
    request: OperatorResolutionRequest,
    ledger: State<'_, Mutex<ValidationLedger>>,
) -> Result<OperatorResolution, String> {
    let ledger = ledger.lock().map_err(|_| "validation ledger lock is poisoned".to_string())?;
    resolve_recorded(request, &ledger).map_err(|error| format!("{error:?}"))
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

pub fn export_context_draft(
    parent_directory: String,
    package_sha256: String,
    new_version: String,
    store: &ContextStore,
) -> Result<ExportedContextPackage, String> {
    let parsed_version = Version::parse(&new_version)
        .map_err(|_| "export version is not Semantic Versioning 2.0.0".to_string())?;
    let parent = PathBuf::from(parent_directory);
    if !parent.is_absolute() || !parent.is_dir() {
        return Err("export parent must be an existing absolute directory".to_string());
    }
    let draft = store.exportable_draft(&package_sha256).map_err(|error| error.to_string())?;
    let descriptor_path =
        parent.join(format!("{}-{parsed_version}", draft.name)).join("datapackage.json");
    let exported = export_package(&draft, &parsed_version.to_string(), &descriptor_path)
        .map_err(|error| error.to_string())?;
    Ok(ExportedContextPackage {
        preview: ContextPackagePreview::from(&exported),
        descriptor_path: descriptor_path.to_string_lossy().into_owned(),
    })
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

#[tauri::command]
fn find_targets_ipc(
    query: String,
    store: State<'_, Mutex<ContextStore>>,
) -> Result<Vec<TargetRecord>, String> {
    store
        .lock()
        .map_err(|_| "context store lock is poisoned".to_string())?
        .find_targets(&query, 25)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_target_draft_ipc(
    package_sha256: String,
    target: AnswerTarget,
    store: State<'_, Mutex<ContextStore>>,
) -> Result<TargetRecord, String> {
    store
        .lock()
        .map_err(|_| "context store lock is poisoned".to_string())?
        .save_target_draft(&package_sha256, target)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn discard_target_draft_ipc(
    package_sha256: String,
    target_id: String,
    store: State<'_, Mutex<ContextStore>>,
) -> Result<bool, String> {
    store
        .lock()
        .map_err(|_| "context store lock is poisoned".to_string())?
        .discard_target_draft(&package_sha256, &target_id)
        .map_err(|error| error.to_string())
}
#[tauri::command]
fn export_context_draft_ipc(
    parent_directory: String,
    package_sha256: String,
    new_version: String,
    store: State<'_, Mutex<ContextStore>>,
) -> Result<ExportedContextPackage, String> {
    let store = store.lock().map_err(|_| "context store lock is poisoned".to_string())?;
    export_context_draft(parent_directory, package_sha256, new_version, &store)
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
            app.manage(Mutex::new(ValidationLedger::default()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            validate,
            resolve,
            inspect_context_package_ipc,
            activate_context_package_ipc,
            current_context_ipc,
            rollback_context_ipc,
            find_targets_ipc,
            save_target_draft_ipc,
            discard_target_draft_ipc,
            export_context_draft_ipc
        ])
        .run(tauri::generate_context!())
        .expect("error while running Semantic Engine");
}

#[cfg(test)]
mod validation_ledger_tests {
    use super::*;
    use semantic_engine_core::{Decision, ResolutionVerdict, ValidationPolicy};

    #[test]
    fn operator_resolution_uses_the_backend_recorded_identity() {
        let round = Round {
            id: "live-round".to_owned(),
            targets: vec![AnswerTarget {
                id: "elden-ring".to_owned(),
                canonical: "Elden Ring".to_owned(),
                aliases: vec![],
            }],
            policy: ValidationPolicy::default(),
        };
        let mut ledger = ValidationLedger::default();
        let validation = validate_and_record(
            round,
            Submission {
                message_id: "chat-44".to_owned(),
                participant_id: "viewer-authentic".to_owned(),
                source_sequence: 44,
                text: "elden ring".to_owned(),
            },
            &mut ledger,
        );
        assert_eq!(validation.decision, Decision::Accepted);

        let resolution = resolve_recorded(
            OperatorResolutionRequest {
                round_id: "live-round".to_owned(),
                message_id: "chat-44".to_owned(),
                verdict: ResolutionVerdict::Accepted,
                target_id: Some("elden-ring".to_owned()),
                note: String::new(),
            },
            &ledger,
        )
        .expect("recorded validation must resolve");
        assert_eq!(resolution.participant_id, "viewer-authentic");
        assert_eq!(resolution.source_sequence, 44);

        let fabricated_round = resolve_recorded(
            OperatorResolutionRequest {
                round_id: "other-round".to_owned(),
                message_id: "chat-44".to_owned(),
                verdict: ResolutionVerdict::Rejected,
                target_id: None,
                note: String::new(),
            },
            &ledger,
        );
        assert_eq!(fabricated_round, Err(ResolutionIssue::ValidationMismatch));
    }
}
