use std::{fs, path::PathBuf, sync::Mutex};

use semantic_engine_context_store::{ContextStore, StoredContext, TargetRecord};
use semantic_engine_core::{
    AnswerTarget, OperatorResolution, OperatorResolutionRequest, Submission, Validation,
};
use semantic_engine_loopback::{
    LoopbackConfig, LoopbackServer, SharedService, start_shared_with_sources as start_loopback,
};
use semantic_engine_package::{ImportedContext, SourceMetadata, export_package, import_package};
use semantic_engine_service::{
    AuditEntry, ResumableSession, SemanticEngineService, ServiceError, SessionEventsPage,
    SessionSnapshot, StartSession,
};
use semantic_engine_source_runtime::SourceRuntime;
use semver::Version;
use serde::Serialize;
use tauri::{Manager, State};

mod sources;

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

struct ActiveLoopback {
    server: LoopbackServer,
    allowed_origins: Vec<String>,
}

struct LoopbackRuntime(tokio::sync::Mutex<Option<ActiveLoopback>>);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LoopbackStatus {
    pub running: bool,
    pub address: Option<String>,
    pub token: Option<String>,
    pub protocol_version: u32,
    pub allowed_origins: Vec<String>,
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
async fn recent_audit_ipc(
    limit: usize,
    service: State<'_, SharedService>,
) -> Result<Vec<AuditEntry>, String> {
    execute_service(service.inner(), move |service| service.recent_audit(limit)).await
}

#[tauri::command]
async fn purge_audit_ipc(service: State<'_, SharedService>) -> Result<usize, String> {
    execute_service(service.inner(), SemanticEngineService::purge_audit).await
}

#[tauri::command]
async fn start_session_ipc(
    request: StartSession,
    service: State<'_, SharedService>,
) -> Result<SessionSnapshot, String> {
    execute_service(service.inner(), move |service| service.start_session(request)).await
}

#[tauri::command]
async fn current_session_ipc(
    session_id: String,
    service: State<'_, SharedService>,
) -> Result<SessionSnapshot, String> {
    execute_service(service.inner(), move |service| service.session(&session_id)).await
}

#[tauri::command]
async fn latest_active_session_ipc(
    service: State<'_, SharedService>,
) -> Result<Option<ResumableSession>, String> {
    execute_service(service.inner(), |service| Ok(service.latest_active_session())).await
}

#[tauri::command]
async fn submit_session_ipc(
    session_id: String,
    submission: Submission,
    service: State<'_, SharedService>,
) -> Result<Validation, String> {
    execute_service(service.inner(), move |service| service.submit(&session_id, submission)).await
}

#[tauri::command]
async fn resolve_session_ipc(
    session_id: String,
    request: OperatorResolutionRequest,
    service: State<'_, SharedService>,
) -> Result<OperatorResolution, String> {
    execute_service(service.inner(), move |service| service.resolve_session(&session_id, request))
        .await
}

#[tauri::command]
async fn end_session_ipc(
    session_id: String,
    service: State<'_, SharedService>,
) -> Result<SessionSnapshot, String> {
    execute_service(service.inner(), move |service| service.end_session(&session_id)).await
}

#[tauri::command]
async fn session_events_ipc(
    session_id: String,
    after_sequence: u64,
    limit: usize,
    service: State<'_, SharedService>,
) -> Result<SessionEventsPage, String> {
    execute_service(service.inner(), move |service| {
        service.session_events(&session_id, after_sequence, limit)
    })
    .await
}

async fn execute_service<T, F>(service: &SharedService, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut SemanticEngineService) -> Result<T, ServiceError> + Send + 'static,
{
    let service = service.clone();
    tokio::task::spawn_blocking(move || operation(&mut service.blocking_lock()))
        .await
        .map_err(|_| "semantic engine service worker did not complete".to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn loopback_status_ipc(
    runtime: State<'_, LoopbackRuntime>,
) -> Result<LoopbackStatus, String> {
    let runtime = runtime.0.lock().await;
    Ok(loopback_status(runtime.as_ref()))
}

#[tauri::command]
async fn start_loopback_ipc(
    port: u16,
    origin: Option<String>,
    service: State<'_, SharedService>,
    sources: State<'_, std::sync::Arc<SourceRuntime>>,
    runtime: State<'_, LoopbackRuntime>,
) -> Result<LoopbackStatus, String> {
    let mut runtime = runtime.0.lock().await;
    if runtime.as_ref().is_some_and(|active| active.server.is_running()) {
        return Ok(loopback_status(runtime.as_ref()));
    }
    if let Some(stale) = runtime.take() {
        stale.server.shutdown().await.map_err(|error| error.to_string())?;
    }
    let mut config = LoopbackConfig::default();
    config.bind_addr.set_port(port);
    if let Some(origin) =
        origin.map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
    {
        config.allowed_origins.push(origin);
    }
    let allowed_origins = config.allowed_origins.clone();
    let server = start_loopback(service.inner().clone(), sources.inner().clone(), config)
        .await
        .map_err(|error| error.to_string())?;
    *runtime = Some(ActiveLoopback { server, allowed_origins });
    Ok(loopback_status(runtime.as_ref()))
}

#[tauri::command]
async fn stop_loopback_ipc(runtime: State<'_, LoopbackRuntime>) -> Result<LoopbackStatus, String> {
    let active = runtime.0.lock().await.take();
    if let Some(active) = active {
        active.server.shutdown().await.map_err(|error| error.to_string())?;
    }
    Ok(loopback_status(None))
}

fn loopback_status(active: Option<&ActiveLoopback>) -> LoopbackStatus {
    let active = active.filter(|active| active.server.is_running());
    LoopbackStatus {
        running: active.is_some(),
        address: active.map(|active| format!("http://{}", active.server.addr())),
        token: active.map(|active| active.server.token().to_owned()),
        protocol_version: semantic_engine_loopback::PROTOCOL_VERSION,
        allowed_origins: active.map_or_else(Vec::new, |active| active.allowed_origins.clone()),
    }
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
            let service = std::sync::Arc::new(tokio::sync::Mutex::new(
                SemanticEngineService::open(data_directory.join("audit.sqlite3"))?,
            ));
            app.manage(Mutex::new(store));
            app.manage(service);
            let service = app.state::<SharedService>().inner().clone();
            app.manage(std::sync::Arc::new(SourceRuntime::open(
                data_directory.join("sources.sqlite3"),
                service,
            )?));
            app.manage(LoopbackRuntime(tokio::sync::Mutex::new(None)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            recent_audit_ipc,
            purge_audit_ipc,
            start_session_ipc,
            current_session_ipc,
            latest_active_session_ipc,
            submit_session_ipc,
            resolve_session_ipc,
            end_session_ipc,
            session_events_ipc,
            loopback_status_ipc,
            start_loopback_ipc,
            stop_loopback_ipc,
            sources::list_sources_ipc,
            sources::create_twitch_source_ipc,
            sources::begin_twitch_authorization_ipc,
            sources::poll_twitch_authorization_ipc,
            sources::test_twitch_source_ipc,
            sources::start_twitch_source_ipc,
            sources::stop_source_ipc,
            sources::delete_source_ipc,
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
