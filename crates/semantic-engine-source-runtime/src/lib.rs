use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use semantic_engine_credential_vault::{CredentialVault, OsCredentialVault};
use semantic_engine_service::{SemanticEngineService, ServiceError};
use semantic_engine_source::{
    CreateSource, SourceAdapterEvent, SourceDesiredState, SourceRecord, SourceRuntimeState,
    SourceStore, UpdateSource,
};
pub use semantic_engine_twitch::DeviceAuthorizationPrompt;
use semantic_engine_twitch::{
    DevicePoll, EventSubConfig, PendingDeviceAuthorization, TWITCH_ADAPTER_ID,
    TwitchEventSubClient, TwitchOAuthClient, load_credential, store_credential,
    validate_twitch_client_id,
};
pub use semantic_engine_youtube::BrowserAuthorizationPrompt;
use semantic_engine_youtube::{
    BrowserPoll, PendingBrowserAuthorization, YOUTUBE_ADAPTER_ID, YouTubeLiveChatClient,
    YouTubeLiveConfig, YouTubeOAuthClient, load_credential as load_youtube_credential,
    store_credential as store_youtube_credential, validate_video_id, validate_youtube_client_id,
};
use serde::Serialize;
use tokio::{
    sync::{RwLock, mpsc, watch},
    task::JoinHandle,
};

const SOURCE_EVENT_BUFFER: usize = 256;
const TOKEN_REFRESH_MARGIN_MS: u64 = 60_000;
const TOKEN_VALIDATION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(55 * 60);
pub const YOUTUBE_DERIVED_DATA_FEATURE_FLAG: &str = "SEMANTIC_ENGINE_ENABLE_YOUTUBE_DERIVED_DATA";

pub type SharedService = Arc<tokio::sync::Mutex<SemanticEngineService>>;

pub struct SourceRuntime {
    store: Arc<Mutex<SourceStore>>,
    vault: Option<Arc<OsCredentialVault>>,
    vault_error: Option<String>,
    oauth: TwitchOAuthClient,
    eventsub: TwitchEventSubClient,
    pending: tokio::sync::Mutex<HashMap<String, PendingDeviceAuthorization>>,
    youtube_oauth: YouTubeOAuthClient,
    youtube_live: YouTubeLiveChatClient,
    pending_youtube: tokio::sync::Mutex<HashMap<String, PendingBrowserAuthorization>>,
    active: tokio::sync::Mutex<HashMap<String, ActiveSource>>,
    runtime: Arc<RwLock<HashMap<String, RuntimeSnapshot>>>,
    global_sequences: Arc<tokio::sync::Mutex<HashMap<String, u64>>>,
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
pub struct YouTubeSourceTest {
    pub channel_id: String,
    pub display_name: String,
    pub video_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TwitchAuthorizationStatus {
    Pending { prompt: DeviceAuthorizationPrompt },
    SlowDown { prompt: DeviceAuthorizationPrompt },
    Authorized { source: Box<SourceView>, identity: TwitchSourceTest },
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum YouTubeAuthorizationStatus {
    Pending { prompt: BrowserAuthorizationPrompt },
    Authorized { source: Box<SourceView>, identity: YouTubeSourceTest },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SourceAuthorizationPrompt {
    Twitch(DeviceAuthorizationPrompt),
    YouTube(BrowserAuthorizationPrompt),
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SourceAuthorizationStatus {
    Twitch(TwitchAuthorizationStatus),
    YouTube(YouTubeAuthorizationStatus),
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum SourceTest {
    Twitch(TwitchSourceTest),
    YouTube(YouTubeSourceTest),
}

impl SourceRuntime {
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
            youtube_oauth: YouTubeOAuthClient::new().map_err(|error| error.to_string())?,
            youtube_live: YouTubeLiveChatClient::new().map_err(|error| error.to_string())?,
            pending_youtube: tokio::sync::Mutex::new(HashMap::new()),
            active: tokio::sync::Mutex::new(HashMap::new()),
            runtime: Arc::new(RwLock::new(HashMap::new())),
            global_sequences: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            service,
        })
    }

    fn vault(&self) -> Result<Arc<OsCredentialVault>, String> {
        self.vault.clone().ok_or_else(|| {
            self.vault_error.clone().unwrap_or_else(|| "OS credential vault unavailable".to_owned())
        })
    }
}

pub async fn list_sources(state: &SourceRuntime) -> Result<Vec<SourceView>, String> {
    list_source_views(state).await
}

pub async fn begin_source_authorization(
    source_id: String,
    state: &SourceRuntime,
) -> Result<SourceAuthorizationPrompt, String> {
    let source = get_source(&state.store, source_id.clone()).await?;
    match source.definition.adapter.as_str() {
        TWITCH_ADAPTER_ID => Ok(SourceAuthorizationPrompt::Twitch(
            begin_twitch_authorization(source_id, state).await?,
        )),
        YOUTUBE_ADAPTER_ID => Ok(SourceAuthorizationPrompt::YouTube(
            begin_youtube_authorization(source_id, state).await?,
        )),
        _ => Err("source adapter does not support authorization".to_owned()),
    }
}

pub async fn poll_source_authorization(
    source_id: String,
    state: &SourceRuntime,
) -> Result<SourceAuthorizationStatus, String> {
    let source = get_source(&state.store, source_id.clone()).await?;
    match source.definition.adapter.as_str() {
        TWITCH_ADAPTER_ID => Ok(SourceAuthorizationStatus::Twitch(
            poll_twitch_authorization(source_id, state).await?,
        )),
        YOUTUBE_ADAPTER_ID => Ok(SourceAuthorizationStatus::YouTube(
            poll_youtube_authorization(source_id, state).await?,
        )),
        _ => Err("source adapter does not support authorization".to_owned()),
    }
}

pub async fn test_source(source_id: String, state: &SourceRuntime) -> Result<SourceTest, String> {
    let source = get_source(&state.store, source_id.clone()).await?;
    match source.definition.adapter.as_str() {
        TWITCH_ADAPTER_ID => Ok(SourceTest::Twitch(test_twitch_source(source_id, state).await?)),
        YOUTUBE_ADAPTER_ID => Ok(SourceTest::YouTube(test_youtube_source(source_id, state).await?)),
        _ => Err("source adapter does not support connection tests".to_owned()),
    }
}

pub async fn start_source(
    source_id: String,
    expected_revision: u64,
    session_id: String,
    state: &SourceRuntime,
) -> Result<SourceView, String> {
    let source = get_source(&state.store, source_id.clone()).await?;
    match source.definition.adapter.as_str() {
        TWITCH_ADAPTER_ID => {
            start_twitch_source(source_id, expected_revision, session_id, state).await
        }
        YOUTUBE_ADAPTER_ID => {
            start_youtube_source(source_id, expected_revision, session_id, state).await
        }
        _ => Err("source adapter cannot be started".to_owned()),
    }
}

pub async fn create_twitch_source(
    display_name: String,
    client_id: String,
    state: &SourceRuntime,
) -> Result<SourceView, String> {
    validate_twitch_client_id(&client_id).map_err(|error| error.to_string())?;
    let source_id = random_source_id("twitch")?;
    let request = CreateSource {
        source_id: source_id.clone(),
        adapter: TWITCH_ADAPTER_ID.to_owned(),
        display_name,
        settings: BTreeMap::from([("client_id".to_owned(), client_id)]),
        credential_id: None,
    };
    execute_store(&state.store, move |store| store.add(request)).await?;
    source_view(state, &source_id).await
}

pub async fn create_youtube_source(
    display_name: String,
    client_id: String,
    video_id: String,
    policy_acknowledged: bool,
    state: &SourceRuntime,
) -> Result<SourceView, String> {
    validate_youtube_client_id(&client_id).map_err(|error| error.to_string())?;
    validate_video_id(&video_id).map_err(|error| error.to_string())?;
    if !policy_acknowledged {
        return Err(
            "YouTube policy acknowledgement is required before enabling this experimental adapter"
                .to_owned(),
        );
    }
    let source_id = random_source_id("youtube")?;
    let request = CreateSource {
        source_id: source_id.clone(),
        adapter: YOUTUBE_ADAPTER_ID.to_owned(),
        display_name,
        settings: BTreeMap::from([
            ("client_id".to_owned(), client_id),
            ("video_id".to_owned(), video_id),
            ("policy_acknowledged".to_owned(), "true".to_owned()),
        ]),
        credential_id: None,
    };
    execute_store(&state.store, move |store| store.add(request)).await?;
    source_view(state, &source_id).await
}

pub async fn begin_twitch_authorization(
    source_id: String,
    state: &SourceRuntime,
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

pub async fn poll_twitch_authorization(
    source_id: String,
    state: &SourceRuntime,
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
            let source = source_view(state, &source_id).await?;
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

pub async fn begin_youtube_authorization(
    source_id: String,
    state: &SourceRuntime,
) -> Result<BrowserAuthorizationPrompt, String> {
    state.vault()?;
    let source = get_source(&state.store, source_id.clone()).await?;
    ensure_youtube(&source)?;
    let client_id = source_setting(&source, "client_id", "YouTube source has no client ID")?;
    let pending = state
        .youtube_oauth
        .begin_authorization(client_id, now_ms()?)
        .await
        .map_err(|error| error.to_string())?;
    let prompt = pending.prompt().clone();
    state.pending_youtube.lock().await.insert(source_id, pending);
    Ok(prompt)
}

pub async fn poll_youtube_authorization(
    source_id: String,
    state: &SourceRuntime,
) -> Result<YouTubeAuthorizationStatus, String> {
    let mut pending = state.pending_youtube.lock().await;
    let authorization = pending
        .get_mut(&source_id)
        .ok_or_else(|| "no pending YouTube authorization for this source".to_owned())?;
    let prompt = authorization.prompt().clone();
    match state
        .youtube_oauth
        .poll_authorization(authorization, now_ms()?)
        .await
        .map_err(|error| error.to_string())?
    {
        BrowserPoll::Pending => Ok(YouTubeAuthorizationStatus::Pending { prompt }),
        BrowserPoll::Authorized(credential) => {
            let source = get_source(&state.store, source_id.clone()).await?;
            let identity = state
                .youtube_oauth
                .identity(credential.access_token())
                .await
                .map_err(|error| error.to_string())?;
            let video_id =
                source_setting(&source, "video_id", "YouTube source has no video ID")?.to_owned();
            state
                .youtube_live
                .test_video(&video_id, credential.access_token())
                .await
                .map_err(|error| error.to_string())?;
            let credential_id = format!("credential-{source_id}");
            let vault = state.vault()?;
            let stored_id = credential_id.clone();
            tokio::task::spawn_blocking(move || {
                store_youtube_credential(vault.as_ref(), &stored_id, &credential)
            })
            .await
            .map_err(|_| "credential worker did not complete".to_owned())?
            .map_err(|error| error.to_string())?;
            let source_id_for_update = source_id.clone();
            let update = UpdateSource {
                expected_revision: source.revision,
                display_name: source.definition.display_name,
                settings: source.definition.settings,
                credential_id: Some(credential_id.clone()),
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
            Ok(YouTubeAuthorizationStatus::Authorized {
                source: Box::new(source_view(state, &source_id).await?),
                identity: YouTubeSourceTest {
                    channel_id: identity.channel_id,
                    display_name: identity.display_name,
                    video_id,
                },
            })
        }
    }
}

pub async fn test_twitch_source(
    source_id: String,
    state: &SourceRuntime,
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

pub async fn test_youtube_source(
    source_id: String,
    state: &SourceRuntime,
) -> Result<YouTubeSourceTest, String> {
    let source = get_source(&state.store, source_id).await?;
    ensure_youtube(&source)?;
    let credential_id = source
        .definition
        .credential_id
        .as_deref()
        .ok_or_else(|| "YouTube authentication is required".to_owned())?;
    let credential = load_youtube_from_vault(state.vault()?, credential_id.to_owned()).await?;
    let identity = state
        .youtube_oauth
        .identity(credential.access_token())
        .await
        .map_err(|error| error.to_string())?;
    let video_id =
        source_setting(&source, "video_id", "YouTube source has no video ID")?.to_owned();
    state
        .youtube_live
        .test_video(&video_id, credential.access_token())
        .await
        .map_err(|error| error.to_string())?;
    Ok(YouTubeSourceTest {
        channel_id: identity.channel_id,
        display_name: identity.display_name,
        video_id,
    })
}

pub async fn start_twitch_source(
    source_id: String,
    expected_revision: u64,
    session_id: String,
    state: &SourceRuntime,
) -> Result<SourceView, String> {
    {
        let active = state.active.lock().await;
        if active.contains_key(&source_id) {
            return source_view(state, &source_id).await;
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
    {
        let mut sequences = state.global_sequences.lock().await;
        sequences.entry(session_id.clone()).or_insert(resumable.next_source_sequence);
    }
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
        state.global_sequences.clone(),
    ));
    state
        .active
        .lock()
        .await
        .insert(source_id.clone(), ActiveSource { session_id, shutdown, connector, pump });
    source_view_with_record(state, active_record).await
}

pub async fn start_youtube_source(
    source_id: String,
    expected_revision: u64,
    session_id: String,
    state: &SourceRuntime,
) -> Result<SourceView, String> {
    {
        let active = state.active.lock().await;
        if active.contains_key(&source_id) {
            return source_view(state, &source_id).await;
        }
    }
    let source = get_source(&state.store, source_id.clone()).await?;
    if source.revision != expected_revision {
        return Err("source revision conflicts with durable state".to_owned());
    }
    ensure_youtube(&source)?;
    if source.definition.settings.get("policy_acknowledged").map(String::as_str) != Some("true") {
        return Err("YouTube policy acknowledgement is required".to_owned());
    }
    if !youtube_derived_data_enabled() {
        return Err(format!(
            "YouTube verdict/score ingestion is distribution-gated; set {YOUTUBE_DERIVED_DATA_FEATURE_FLAG}=1 only for an approved compliance test"
        ));
    }
    let client_id =
        source_setting(&source, "client_id", "YouTube source has no client ID")?.to_owned();
    let video_id =
        source_setting(&source, "video_id", "YouTube source has no video ID")?.to_owned();
    let credential_id = source
        .definition
        .credential_id
        .clone()
        .ok_or_else(|| "YouTube authentication is required".to_owned())?;
    let vault = state.vault()?;
    let mut credential = load_youtube_from_vault(vault.clone(), credential_id.clone()).await?;
    if credential.expires_at_ms() <= now_ms()?.saturating_add(TOKEN_REFRESH_MARGIN_MS) {
        credential = state
            .youtube_oauth
            .refresh(&client_id, &credential, now_ms()?)
            .await
            .map_err(|error| error.to_string())?;
        let stored = credential_id.clone();
        let vault_for_store = vault.clone();
        credential = tokio::task::spawn_blocking(move || {
            store_youtube_credential(vault_for_store.as_ref(), &stored, &credential)
                .map(|()| credential)
        })
        .await
        .map_err(|_| "credential worker did not complete".to_owned())?
        .map_err(|error| error.to_string())?;
    }
    state
        .youtube_oauth
        .identity(credential.access_token())
        .await
        .map_err(|error| error.to_string())?;
    state
        .youtube_live
        .test_video(&video_id, credential.access_token())
        .await
        .map_err(|error| error.to_string())?;
    let resumable = execute_service(&state.service, {
        let session_id = session_id.clone();
        move |service| service.resumable_session(&session_id)
    })
    .await?;
    state
        .global_sequences
        .lock()
        .await
        .entry(session_id.clone())
        .or_insert(resumable.next_source_sequence);
    let source_id_for_state = source_id.clone();
    let active_record = execute_store(&state.store, move |store| {
        store.set_desired_state(&source_id_for_state, expected_revision, SourceDesiredState::Active)
    })
    .await?;
    let (output, events) = mpsc::channel(SOURCE_EVENT_BUFFER);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let connector = {
        let oauth = state.youtube_oauth.clone();
        let live = state.youtube_live.clone();
        let runtime = state.runtime.clone();
        let connector_source_id = source_id.clone();
        let config = YouTubeLiveConfig {
            source_id: source_id.clone(),
            video_id,
            next_source_sequence: resumable.next_source_sequence,
        };
        tokio::spawn(async move {
            if run_youtube_supervisor(
                oauth,
                live,
                config,
                client_id,
                vault,
                credential_id,
                credential,
                output,
                shutdown_receiver,
            )
            .await
            .is_err()
            {
                set_runtime_fault(&runtime, &connector_source_id, "connector_failed").await;
            }
        })
    };
    let pump = tokio::spawn(pump_source_events(
        source_id.clone(),
        session_id.clone(),
        events,
        state.service.clone(),
        state.runtime.clone(),
        state.global_sequences.clone(),
    ));
    state
        .active
        .lock()
        .await
        .insert(source_id.clone(), ActiveSource { session_id, shutdown, connector, pump });
    source_view_with_record(state, active_record).await
}

#[allow(clippy::too_many_arguments)]
async fn run_youtube_supervisor(
    oauth: YouTubeOAuthClient,
    live: YouTubeLiveChatClient,
    config: YouTubeLiveConfig,
    client_id: String,
    vault: Arc<OsCredentialVault>,
    credential_id: String,
    mut credential: semantic_engine_youtube::YouTubeCredential,
    output: mpsc::Sender<SourceAdapterEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let mut force_refresh = false;
    let mut backoff = std::time::Duration::from_secs(1);
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let now = now_ms()?;
        if force_refresh
            || credential.expires_at_ms() <= now.saturating_add(TOKEN_REFRESH_MARGIN_MS)
        {
            credential = oauth
                .refresh(&client_id, &credential, now)
                .await
                .map_err(|error| error.to_string())?;
            let stored_id = credential_id.clone();
            let stored_vault = vault.clone();
            credential = tokio::task::spawn_blocking(move || {
                store_youtube_credential(stored_vault.as_ref(), &stored_id, &credential)
                    .map(|()| credential)
            })
            .await
            .map_err(|_| "credential worker did not complete".to_owned())?
            .map_err(|error| error.to_string())?;
            force_refresh = false;
            backoff = std::time::Duration::from_secs(1);
        }
        let until_refresh = std::time::Duration::from_millis(
            credential
                .expires_at_ms()
                .saturating_sub(now_ms()?)
                .saturating_sub(TOKEN_REFRESH_MARGIN_MS),
        );
        let (cycle_shutdown, cycle_receiver) = watch::channel(false);
        let cycle = live.run(config.clone(), &credential, output.clone(), cycle_receiver);
        tokio::pin!(cycle);
        tokio::select! {
            result = &mut cycle => {
                match result {
                    Ok(()) if *shutdown.borrow() => return Ok(()),
                    Ok(()) => continue,
                    Err(semantic_engine_youtube::YouTubeError::Api { status: 401, .. }) => {
                        force_refresh = true;
                    }
                    Err(semantic_engine_youtube::YouTubeError::Transport(_))
                    | Err(semantic_engine_youtube::YouTubeError::Api { status: 429 | 500..=599, .. }) => {
                        let _ = output.try_send(SourceAdapterEvent::StateChanged {
                            source_id: config.source_id.clone(),
                            state: SourceRuntimeState::Backoff,
                            detail: Some(format!("retry in {}s", backoff.as_secs())),
                        });
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            changed = shutdown.changed() => {
                                if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                            }
                        }
                        backoff = (backoff * 2).min(std::time::Duration::from_secs(60));
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            _ = tokio::time::sleep(until_refresh) => {
                let _ = cycle_shutdown.send(true);
                let _ = cycle.await;
            }
            changed = shutdown.changed() => {
                let _ = cycle_shutdown.send(true);
                let _ = cycle.await;
                if changed.is_err() || *shutdown.borrow() { return Ok(()); }
            }
        }
    }
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

pub async fn stop_source(source_id: String, state: &SourceRuntime) -> Result<SourceView, String> {
    let mut stopped_session = None;
    if let Some(active) = state.active.lock().await.remove(&source_id) {
        stopped_session = Some(active.session_id.clone());
        let _ = active.shutdown.send(true);
        let _ = active.connector.await;
        let _ = active.pump.await;
    }
    if let Some(session_id) = stopped_session {
        let session_still_active =
            state.active.lock().await.values().any(|source| source.session_id == session_id);
        if !session_still_active {
            state.global_sequences.lock().await.remove(&session_id);
        }
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
    source_view_with_record(state, paused).await
}

pub async fn delete_source(
    source_id: String,
    expected_revision: u64,
    state: &SourceRuntime,
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
        if source.definition.adapter == YOUTUBE_ADAPTER_ID
            && let Ok(credential) =
                load_youtube_from_vault(vault.clone(), credential_id.clone()).await
        {
            let _ = state.youtube_oauth.revoke(credential.access_token()).await;
        }
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
    state.pending_youtube.lock().await.remove(&source_id);
    state.runtime.write().await.remove(&source_id);
    Ok(())
}

async fn pump_source_events(
    source_id: String,
    session_id: String,
    mut events: mpsc::Receiver<SourceAdapterEvent>,
    service: SharedService,
    runtime: Arc<RwLock<HashMap<String, RuntimeSnapshot>>>,
    global_sequences: Arc<tokio::sync::Mutex<HashMap<String, u64>>>,
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
                let mut submission = message.into_submission();
                let mut sequences = global_sequences.lock().await;
                let next_sequence = sequences.entry(session_id.clone()).or_default();
                submission.source_sequence = *next_sequence;
                let validation = execute_service(&service, {
                    let session_id = session_id.clone();
                    move |service| service.submit(&session_id, submission)
                })
                .await;
                if validation.is_ok() {
                    *next_sequence = next_sequence.saturating_add(1);
                }
                drop(sequences);
                let accepted = validation.is_ok_and(|validation| {
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

async fn list_source_views(state: &SourceRuntime) -> Result<Vec<SourceView>, String> {
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

async fn source_view(state: &SourceRuntime, source_id: &str) -> Result<SourceView, String> {
    let record = get_source(&state.store, source_id.to_owned()).await?;
    source_view_with_record(state, record).await
}

async fn source_view_with_record(
    state: &SourceRuntime,
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

async fn load_youtube_from_vault(
    vault: Arc<OsCredentialVault>,
    credential_id: String,
) -> Result<semantic_engine_youtube::YouTubeCredential, String> {
    tokio::task::spawn_blocking(move || load_youtube_credential(vault.as_ref(), &credential_id))
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

fn ensure_youtube(source: &SourceRecord) -> Result<(), String> {
    if source.definition.adapter != YOUTUBE_ADAPTER_ID {
        return Err("source is not a YouTube Live adapter".to_owned());
    }
    Ok(())
}

fn source_setting<'a>(
    source: &'a SourceRecord,
    key: &str,
    missing: &str,
) -> Result<&'a str, String> {
    source.definition.settings.get(key).map(String::as_str).ok_or_else(|| missing.to_owned())
}

fn source_client_id(source: &SourceRecord) -> Result<&str, String> {
    source
        .definition
        .settings
        .get("client_id")
        .map(String::as_str)
        .ok_or_else(|| "Twitch source has no client ID".to_owned())
}

fn random_source_id(prefix: &str) -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "OS randomness is unavailable".to_owned())?;
    Ok(format!("{prefix}-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
}

fn youtube_derived_data_enabled() -> bool {
    std::env::var(YOUTUBE_DERIVED_DATA_FEATURE_FLAG).is_ok_and(|value| value == "1")
}

fn now_ms() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_owned())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "system time cannot be represented".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use semantic_engine_core::{AnswerTarget, Round, ValidationPolicy};
    use semantic_engine_service::{SessionEventKind, StartSession};
    use semantic_engine_source::SourceMessage;

    fn message(source_id: &str, message_id: &str) -> SourceAdapterEvent {
        SourceAdapterEvent::Message(SourceMessage {
            source_id: source_id.to_owned(),
            message_id: format!("{source_id}:{message_id}"),
            participant_id: format!("{source_id}:viewer"),
            source_sequence: 99,
            text: "Elden Ring".to_owned(),
            occurred_at_ms: 1,
        })
    }

    #[tokio::test]
    async fn multiple_source_pumps_share_one_durable_session_order() {
        let mut service = SemanticEngineService::in_memory().expect("service");
        service
            .start_session(StartSession {
                session_id: "session-1".to_owned(),
                round: Round {
                    id: "round-1".to_owned(),
                    targets: vec![AnswerTarget {
                        id: "elden-ring".to_owned(),
                        canonical: "Elden Ring".to_owned(),
                        aliases: Vec::new(),
                    }],
                    policy: ValidationPolicy::default(),
                },
                context_package_sha256: None,
            })
            .expect("session");
        let service = Arc::new(tokio::sync::Mutex::new(service));
        let runtime = Arc::new(RwLock::new(HashMap::new()));
        let sequences =
            Arc::new(tokio::sync::Mutex::new(HashMap::from([("session-1".to_owned(), 0)])));
        let (left_tx, left_rx) = mpsc::channel(2);
        let (right_tx, right_rx) = mpsc::channel(2);
        let left = tokio::spawn(pump_source_events(
            "left".to_owned(),
            "session-1".to_owned(),
            left_rx,
            service.clone(),
            runtime.clone(),
            sequences.clone(),
        ));
        let right = tokio::spawn(pump_source_events(
            "right".to_owned(),
            "session-1".to_owned(),
            right_rx,
            service.clone(),
            runtime,
            sequences,
        ));

        left_tx.send(message("left", "1")).await.expect("left message");
        right_tx.send(message("right", "1")).await.expect("right message");
        drop(left_tx);
        drop(right_tx);
        left.await.expect("left pump");
        right.await.expect("right pump");

        let service = service.lock().await;
        let events = service.session_events("session-1", 0, 10).expect("events");
        let source_sequences = events
            .events
            .iter()
            .filter_map(|event| match &event.kind {
                SessionEventKind::ValidationRecorded(validation) => {
                    Some(validation.source_sequence)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(source_sequences, vec![0, 1]);
        assert_eq!(
            service.resumable_session("session-1").expect("resumable").next_source_sequence,
            2
        );
    }
}
