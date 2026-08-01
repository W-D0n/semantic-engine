use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use semantic_engine_credential_vault::{CredentialVault, OsCredentialVault};
use semantic_engine_loopback::SharedService;
use semantic_engine_service::{SemanticEngineService, ServiceError};
use semantic_engine_source::{
    CreateSource, SourceAdapterEvent, SourceDesiredState, SourceRecord, SourceRuntimeState,
    SourceStore, UpdateSource,
};
use semantic_engine_twitch::{
    DeviceAuthorizationPrompt, DevicePoll, EventSubConfig, PendingDeviceAuthorization,
    TWITCH_ADAPTER_ID, TwitchEventSubClient, TwitchOAuthClient, load_credential, store_credential,
    validate_twitch_client_id,
};
use serde::Serialize;
use tauri::State;
use tokio::{
    sync::{RwLock, mpsc, watch},
    task::JoinHandle,
};

const SOURCE_EVENT_BUFFER: usize = 256;
const TOKEN_REFRESH_MARGIN_MS: u64 = 60_000;
const TOKEN_VALIDATION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(55 * 60);

pub struct SourceAppState {
    store: Arc<Mutex<SourceStore>>,
    vault: Option<Arc<OsCredentialVault>>,
    vault_error: Option<String>,
    oauth: TwitchOAuthClient,
    eventsub: TwitchEventSubClient,
    pending: tokio::sync::Mutex<HashMap<String, PendingDeviceAuthorization>>,
    active: tokio::sync::Mutex<HashMap<String, ActiveSource>>,
    runtime: Arc<RwLock<HashMap<String, RuntimeSnapshot>>>,
    service: SharedService,
}

struct ActiveSource {
    session_id: String,
    shutdown: watch::Sender<bool>,
    connector: JoinHandle<()>,
    pump: JoinHandle<()>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RuntimeSnapshot {
    pub state: Option<SourceRuntimeState>,
    pub detail: Option<String>,
    pub session_id: Option<String>,
    pub messages_received: u64,
    pub accepted: u64,
    pub last_event_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceView {
    #[serde(flatten)]
    pub record: SourceRecord,
    pub runtime: RuntimeSnapshot,
    pub authenticated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TwitchSourceTest {
    pub login: String,
    pub user_id: String,
    pub expires_in_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TwitchAuthorizationStatus {
    Pending { prompt: DeviceAuthorizationPrompt },
    SlowDown { prompt: DeviceAuthorizationPrompt },
    Authorized { source: Box<SourceView>, identity: TwitchSourceTest },
}

impl SourceAppState {
    pub fn open(path: std::path::PathBuf, service: SharedService) -> Result<Self, String> {
        let mut store = SourceStore::open(path).map_err(|error| error.to_string())?;
        for source in store.list().map_err(|error| error.to_string())? {
            if source.desired_state == SourceDesiredState::Active {
                store
                    .set_desired_state(
                        &source.definition.source_id,
                        source.revision,
                        SourceDesiredState::Paused,
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        let (vault, vault_error) = match OsCredentialVault::semantic_engine() {
            Ok(vault) => (Some(Arc::new(vault)), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            vault,
            vault_error,
            oauth: TwitchOAuthClient::new().map_err(|error| error.to_string())?,
            eventsub: TwitchEventSubClient::new().map_err(|error| error.to_string())?,
            pending: tokio::sync::Mutex::new(HashMap::new()),
            active: tokio::sync::Mutex::new(HashMap::new()),
            runtime: Arc::new(RwLock::new(HashMap::new())),
            service,
        })
    }

    fn vault(&self) -> Result<Arc<OsCredentialVault>, String> {
        self.vault.clone().ok_or_else(|| {
            self.vault_error.clone().unwrap_or_else(|| "OS credential vault unavailable".to_owned())
        })
    }
}

#[tauri::command]
pub async fn list_sources_ipc(state: State<'_, SourceAppState>) -> Result<Vec<SourceView>, String> {
    list_source_views(&state).await
}

#[tauri::command]
pub async fn create_twitch_source_ipc(
    display_name: String,
    client_id: String,
    state: State<'_, SourceAppState>,
) -> Result<SourceView, String> {
    validate_twitch_client_id(&client_id).map_err(|error| error.to_string())?;
    let source_id = random_source_id()?;
    let request = CreateSource {
        source_id: source_id.clone(),
        adapter: TWITCH_ADAPTER_ID.to_owned(),
        display_name,
        settings: BTreeMap::from([("client_id".to_owned(), client_id)]),
        credential_id: None,
    };
    execute_store(&state.store, move |store| store.add(request)).await?;
    source_view(&state, &source_id).await
}

#[tauri::command]
pub async fn begin_twitch_authorization_ipc(
    source_id: String,
    state: State<'_, SourceAppState>,
) -> Result<DeviceAuthorizationPrompt, String> {
    state.vault()?;
    let source = get_source(&state.store, source_id.clone()).await?;
    ensure_twitch(&source)?;
    let client_id = source_client_id(&source)?;
    let pending = state
        .oauth
        .begin_device_authorization(client_id, now_ms()?)
        .await
        .map_err(|error| error.to_string())?;
    let prompt = pending.prompt().clone();
    state.pending.lock().await.insert(source_id, pending);
    Ok(prompt)
}

#[tauri::command]
pub async fn poll_twitch_authorization_ipc(
    source_id: String,
    state: State<'_, SourceAppState>,
) -> Result<TwitchAuthorizationStatus, String> {
    let mut pending = state.pending.lock().await;
    let authorization = pending
        .get(&source_id)
        .ok_or_else(|| "no pending Twitch authorization for this source".to_owned())?;
    let prompt = authorization.prompt().clone();
    match state
        .oauth
        .poll_device_authorization(authorization, now_ms()?)
        .await
        .map_err(|error| error.to_string())?
    {
        DevicePoll::Pending => Ok(TwitchAuthorizationStatus::Pending { prompt }),
        DevicePoll::SlowDown => Ok(TwitchAuthorizationStatus::SlowDown { prompt }),
        DevicePoll::Authorized(credential) => {
            let source = get_source(&state.store, source_id.clone()).await?;
            let client_id = source_client_id(&source)?.to_owned();
            let identity = state
                .oauth
                .validate(credential.access_token())
                .await
                .map_err(|error| error.to_string())?;
            if identity.client_id != client_id {
                return Err("Twitch returned a token for a different client ID".to_owned());
            }
            let credential_id = format!("credential-{source_id}");
            let vault = state.vault()?;
            let stored_id = credential_id.clone();
            tokio::task::spawn_blocking(move || {
                store_credential(vault.as_ref(), &stored_id, &credential)
            })
            .await
            .map_err(|_| "credential worker did not complete".to_owned())?
            .map_err(|error| error.to_string())?;
            let updated_id = credential_id.clone();
            let source_id_for_update = source_id.clone();
            let update = UpdateSource {
                expected_revision: source.revision,
                display_name: source.definition.display_name,
                settings: source.definition.settings,
                credential_id: Some(updated_id),
            };
            if let Err(error) = execute_store(&state.store, move |store| {
                store.update(&source_id_for_update, update)
            })
            .await
            {
                let _ = state.vault()?.delete(&credential_id);
                return Err(error);
            }
            pending.remove(&source_id);
            drop(pending);
            let source = source_view(&state, &source_id).await?;
            Ok(TwitchAuthorizationStatus::Authorized {
                source: Box::new(source),
                identity: TwitchSourceTest {
                    login: identity.login,
                    user_id: identity.user_id,
                    expires_in_seconds: identity.expires_in,
                },
            })
        }
    }
}

#[tauri::command]
pub async fn test_twitch_source_ipc(
    source_id: String,
    state: State<'_, SourceAppState>,
) -> Result<TwitchSourceTest, String> {
    let source = get_source(&state.store, source_id).await?;
    ensure_twitch(&source)?;
    let client_id = source_client_id(&source)?;
    let credential_id = source
        .definition
        .credential_id
        .as_deref()
        .ok_or_else(|| "Twitch authentication is required".to_owned())?;
    let credential = load_from_vault(state.vault()?, credential_id.to_owned()).await?;
    let identity =
        state.oauth.validate(credential.access_token()).await.map_err(|error| error.to_string())?;
    if identity.client_id != client_id {
        return Err("stored Twitch token belongs to a different client ID".to_owned());
    }
    Ok(TwitchSourceTest {
        login: identity.login,
        user_id: identity.user_id,
        expires_in_seconds: identity.expires_in,
    })
}

#[tauri::command]
pub async fn start_twitch_source_ipc(
    source_id: String,
    expected_revision: u64,
    session_id: String,
    state: State<'_, SourceAppState>,
) -> Result<SourceView, String> {
    {
        let active = state.active.lock().await;
        if active.contains_key(&source_id) {
            return source_view(&state, &source_id).await;
        }
        if active.values().any(|source| source.session_id == session_id) {
            return Err("only one live source can feed a session in contract v1".to_owned());
        }
    }
    let source = get_source(&state.store, source_id.clone()).await?;
    if source.revision != expected_revision {
        return Err("source revision conflicts with durable state".to_owned());
    }
    ensure_twitch(&source)?;
    let client_id = source_client_id(&source)?.to_owned();
    let credential_id = source
        .definition
        .credential_id
        .clone()
        .ok_or_else(|| "Twitch authentication is required".to_owned())?;
    let vault = state.vault()?;
    let mut credential = load_from_vault(vault.clone(), credential_id.clone()).await?;
    if credential.expires_at_ms() <= now_ms()?.saturating_add(TOKEN_REFRESH_MARGIN_MS) {
        credential = state
            .oauth
            .refresh(&client_id, credential.refresh_token(), now_ms()?)
            .await
            .map_err(|error| error.to_string())?;
        let stored = credential_id.clone();
        let vault_for_store = vault.clone();
        credential = tokio::task::spawn_blocking(move || {
            store_credential(vault_for_store.as_ref(), &stored, &credential).map(|()| credential)
        })
        .await
        .map_err(|_| "credential worker did not complete".to_owned())?
        .map_err(|error| error.to_string())?;
    }
    let identity =
        state.oauth.validate(credential.access_token()).await.map_err(|error| error.to_string())?;
    if identity.client_id != client_id {
        return Err("stored Twitch token belongs to a different client ID".to_owned());
    }
    let resumable = execute_service(&state.service, {
        let session_id = session_id.clone();
        move |service| service.resumable_session(&session_id)
    })
    .await?;
    let source_id_for_state = source_id.clone();
    let active_record = execute_store(&state.store, move |store| {
        store.set_desired_state(&source_id_for_state, expected_revision, SourceDesiredState::Active)
    })
    .await?;
    let (output, events) = mpsc::channel(SOURCE_EVENT_BUFFER);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let eventsub = state.eventsub.clone();
    let oauth = state.oauth.clone();
    let eventsub_config = EventSubConfig {
        source_id: source_id.clone(),
        client_id,
        broadcaster_user_id: identity.user_id.clone(),
        user_id: identity.user_id,
        next_source_sequence: resumable.next_source_sequence,
    };
    let runtime_for_connector = state.runtime.clone();
    let connector_source_id = source_id.clone();
    let connector_vault = vault;
    let connector_credential_id = credential_id;
    let connector = tokio::spawn(async move {
        if run_twitch_supervisor(
            oauth,
            eventsub,
            eventsub_config,
            connector_vault,
            connector_credential_id,
            credential,
            output,
            shutdown_receiver,
        )
        .await
        .is_err()
        {
            set_runtime_fault(&runtime_for_connector, &connector_source_id, "connector_failed")
                .await;
        }
    });
    let pump = tokio::spawn(pump_source_events(
        source_id.clone(),
        session_id.clone(),
        events,
        state.service.clone(),
        state.runtime.clone(),
    ));
    state
        .active
        .lock()
        .await
        .insert(source_id.clone(), ActiveSource { session_id, shutdown, connector, pump });
    source_view_with_record(&state, active_record).await
}

#[allow(clippy::too_many_arguments)]
async fn run_twitch_supervisor(
    oauth: TwitchOAuthClient,
    eventsub: TwitchEventSubClient,
    config: EventSubConfig,
    vault: Arc<OsCredentialVault>,
    credential_id: String,
    mut credential: semantic_engine_twitch::TwitchCredential,
    output: mpsc::Sender<SourceAdapterEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let mut force_refresh = false;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let now = now_ms()?;
        if force_refresh
            || credential.expires_at_ms() <= now.saturating_add(TOKEN_REFRESH_MARGIN_MS)
        {
            credential = oauth
                .refresh(&config.client_id, credential.refresh_token(), now)
                .await
                .map_err(|error| error.to_string())?;
            let stored_id = credential_id.clone();
            let stored_vault = vault.clone();
            credential = tokio::task::spawn_blocking(move || {
                store_credential(stored_vault.as_ref(), &stored_id, &credential)
                    .map(|()| credential)
            })
            .await
            .map_err(|_| "credential worker did not complete".to_owned())?
            .map_err(|error| error.to_string())?;
            force_refresh = false;
        }
        let identity =
            oauth.validate(credential.access_token()).await.map_err(|error| error.to_string())?;
        if identity.client_id != config.client_id || identity.user_id != config.user_id {
            return Err("validated Twitch identity changed".to_owned());
        }
        let until_refresh = std::time::Duration::from_millis(
            credential
                .expires_at_ms()
                .saturating_sub(now_ms()?)
                .saturating_sub(TOKEN_REFRESH_MARGIN_MS),
        );
        let supervision_interval = TOKEN_VALIDATION_INTERVAL.min(until_refresh);
        let (cycle_shutdown, cycle_receiver) = watch::channel(false);
        let cycle = eventsub.run(config.clone(), &credential, output.clone(), cycle_receiver);
        tokio::pin!(cycle);
        tokio::select! {
            result = &mut cycle => {
                match result {
                    Ok(()) if *shutdown.borrow() => return Ok(()),
                    Ok(()) => continue,
                    Err(semantic_engine_twitch::TwitchError::Api { status: 401, .. }) => {
                        force_refresh = true;
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            _ = tokio::time::sleep(supervision_interval) => {
                let _ = cycle_shutdown.send(true);
                let _ = cycle.await;
            }
            changed = shutdown.changed() => {
                let _ = cycle_shutdown.send(true);
                let _ = cycle.await;
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

#[tauri::command]
pub async fn stop_source_ipc(
    source_id: String,
    state: State<'_, SourceAppState>,
) -> Result<SourceView, String> {
    if let Some(active) = state.active.lock().await.remove(&source_id) {
        let _ = active.shutdown.send(true);
        let _ = active.connector.await;
        let _ = active.pump.await;
    }
    let current = get_source(&state.store, source_id.clone()).await?;
    let source_id_for_pause = source_id.clone();
    let paused = execute_store(&state.store, move |store| {
        store.set_desired_state(&source_id_for_pause, current.revision, SourceDesiredState::Paused)
    })
    .await?;
    state.runtime.write().await.insert(
        source_id,
        RuntimeSnapshot { state: Some(SourceRuntimeState::Paused), ..RuntimeSnapshot::default() },
    );
    source_view_with_record(&state, paused).await
}

#[tauri::command]
pub async fn delete_source_ipc(
    source_id: String,
    expected_revision: u64,
    state: State<'_, SourceAppState>,
) -> Result<(), String> {
    if state.active.lock().await.contains_key(&source_id) {
        return Err("pause the source before removing it".to_owned());
    }
    let source = get_source(&state.store, source_id.clone()).await?;
    if source.revision != expected_revision || source.desired_state != SourceDesiredState::Paused {
        return Err("source changed or is not paused".to_owned());
    }
    if let Some(credential_id) = source.definition.credential_id {
        let vault = state.vault()?;
        tokio::task::spawn_blocking(move || vault.delete(&credential_id))
            .await
            .map_err(|_| "credential worker did not complete".to_owned())?
            .map_err(|error| error.to_string())?;
    }
    let source_id_for_remove = source_id.clone();
    execute_store(&state.store, move |store| {
        store.remove(&source_id_for_remove, expected_revision)
    })
    .await?;
    state.pending.lock().await.remove(&source_id);
    state.runtime.write().await.remove(&source_id);
    Ok(())
}

async fn pump_source_events(
    source_id: String,
    session_id: String,
    mut events: mpsc::Receiver<SourceAdapterEvent>,
    service: SharedService,
    runtime: Arc<RwLock<HashMap<String, RuntimeSnapshot>>>,
) {
    while let Some(event) = events.recv().await {
        match event {
            SourceAdapterEvent::StateChanged { state, detail, .. } => {
                let mut snapshots = runtime.write().await;
                let snapshot = snapshots.entry(source_id.clone()).or_default();
                snapshot.state = Some(state);
                snapshot.detail = detail;
                snapshot.session_id = Some(session_id.clone());
                snapshot.last_event_at_ms = now_ms().ok();
            }
            SourceAdapterEvent::Message(message) => {
                let submission = message.into_submission();
                let accepted = execute_service(&service, {
                    let session_id = session_id.clone();
                    move |service| service.submit(&session_id, submission)
                })
                .await
                .is_ok_and(|validation| {
                    validation.decision == semantic_engine_core::Decision::Accepted
                });
                let mut snapshots = runtime.write().await;
                let snapshot = snapshots.entry(source_id.clone()).or_default();
                snapshot.messages_received = snapshot.messages_received.saturating_add(1);
                snapshot.accepted = snapshot.accepted.saturating_add(u64::from(accepted));
                snapshot.last_event_at_ms = now_ms().ok();
            }
            SourceAdapterEvent::Fault { code, .. } => {
                set_runtime_fault(&runtime, &source_id, &code).await;
            }
        }
    }
}

async fn set_runtime_fault(
    runtime: &Arc<RwLock<HashMap<String, RuntimeSnapshot>>>,
    source_id: &str,
    code: &str,
) {
    let mut snapshots = runtime.write().await;
    let snapshot = snapshots.entry(source_id.to_owned()).or_default();
    snapshot.state = Some(SourceRuntimeState::Faulted);
    snapshot.detail = Some(code.to_owned());
    snapshot.last_event_at_ms = now_ms().ok();
}

async fn list_source_views(state: &SourceAppState) -> Result<Vec<SourceView>, String> {
    let records = execute_store(&state.store, |store| store.list()).await?;
    let runtime = state.runtime.read().await;
    Ok(records
        .into_iter()
        .map(|record| {
            let snapshot =
                runtime.get(&record.definition.source_id).cloned().unwrap_or_else(|| {
                    RuntimeSnapshot {
                        state: Some(if record.definition.credential_id.is_some() {
                            SourceRuntimeState::Paused
                        } else {
                            SourceRuntimeState::AuthenticationRequired
                        }),
                        ..RuntimeSnapshot::default()
                    }
                });
            SourceView {
                authenticated: record.definition.credential_id.is_some(),
                record,
                runtime: snapshot,
            }
        })
        .collect())
}

async fn source_view(state: &SourceAppState, source_id: &str) -> Result<SourceView, String> {
    let record = get_source(&state.store, source_id.to_owned()).await?;
    source_view_with_record(state, record).await
}

async fn source_view_with_record(
    state: &SourceAppState,
    record: SourceRecord,
) -> Result<SourceView, String> {
    let runtime =
        state.runtime.read().await.get(&record.definition.source_id).cloned().unwrap_or_else(
            || RuntimeSnapshot {
                state: Some(if record.definition.credential_id.is_some() {
                    SourceRuntimeState::Paused
                } else {
                    SourceRuntimeState::AuthenticationRequired
                }),
                ..RuntimeSnapshot::default()
            },
        );
    Ok(SourceView { authenticated: record.definition.credential_id.is_some(), record, runtime })
}

async fn get_source(
    store: &Arc<Mutex<SourceStore>>,
    source_id: String,
) -> Result<SourceRecord, String> {
    execute_store(store, move |store| store.get(&source_id)).await
}

async fn execute_store<T, F>(store: &Arc<Mutex<SourceStore>>, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut SourceStore) -> Result<T, semantic_engine_source::SourceError> + Send + 'static,
{
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = store.lock().map_err(|_| "source store lock is poisoned".to_owned())?;
        operation(&mut store).map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "source store worker did not complete".to_owned())?
}

async fn execute_service<T, F>(service: &SharedService, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut SemanticEngineService) -> Result<T, ServiceError> + Send + 'static,
{
    let service = service.clone();
    tokio::task::spawn_blocking(move || operation(&mut service.blocking_lock()))
        .await
        .map_err(|_| "semantic engine service worker did not complete".to_owned())?
        .map_err(|error| error.to_string())
}

async fn load_from_vault(
    vault: Arc<OsCredentialVault>,
    credential_id: String,
) -> Result<semantic_engine_twitch::TwitchCredential, String> {
    tokio::task::spawn_blocking(move || load_credential(vault.as_ref(), &credential_id))
        .await
        .map_err(|_| "credential worker did not complete".to_owned())?
        .map_err(|error| error.to_string())
}

fn ensure_twitch(source: &SourceRecord) -> Result<(), String> {
    if source.definition.adapter != TWITCH_ADAPTER_ID {
        return Err("source is not a Twitch EventSub adapter".to_owned());
    }
    Ok(())
}

fn source_client_id(source: &SourceRecord) -> Result<&str, String> {
    source
        .definition
        .settings
        .get("client_id")
        .map(String::as_str)
        .ok_or_else(|| "Twitch source has no client ID".to_owned())
}

fn random_source_id() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "OS randomness is unavailable".to_owned())?;
    Ok(format!("twitch-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
}

fn now_ms() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_owned())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "system time cannot be represented".to_owned())
}
