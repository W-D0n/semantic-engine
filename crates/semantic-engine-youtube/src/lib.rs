use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use semantic_engine_core::{MAX_IDENTIFIER_CHARS, MAX_SUBMISSION_CHARS};
use semantic_engine_credential_vault::{CredentialVault, VaultError};
use semantic_engine_source::{SourceAdapterEvent, SourceMessage, SourceRuntimeState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tonic::{
    Code, Request,
    metadata::MetadataValue,
    transport::{Channel as GrpcChannel, ClientTlsConfig, Endpoint},
};
use zeroize::Zeroize;

pub const YOUTUBE_ADAPTER_ID: &str = "youtube-live-chat";
pub const READONLY_SCOPE: &str = "https://www.googleapis.com/auth/youtube.readonly";

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
const API_ENDPOINT: &str = "https://www.googleapis.com/youtube/v3/";
const GRPC_ENDPOINT: &str = "https://youtube.googleapis.com";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_TOKEN_CHARS: usize = 8_192;
const MAX_CALLBACK_BYTES: usize = 8 * 1024;
const AUTH_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_RECENT_MESSAGE_IDS: usize = 4_096;

mod stream_api {
    tonic::include_proto!("youtube.api.v3");
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BrowserAuthorizationPrompt {
    pub authorization_uri: String,
    pub expires_at_ms: u64,
}

pub struct PendingBrowserAuthorization {
    client_id: String,
    verifier: SecretString,
    redirect_uri: String,
    prompt: BrowserAuthorizationPrompt,
    callback: Option<oneshot::Receiver<Result<String, YouTubeError>>>,
    authorization_code: Option<SecretString>,
}

impl PendingBrowserAuthorization {
    #[must_use]
    pub fn prompt(&self) -> &BrowserAuthorizationPrompt {
        &self.prompt
    }

    #[must_use]
    pub fn is_expired_at(&self, now_ms: u64) -> bool {
        now_ms >= self.prompt.expires_at_ms
    }
}

impl fmt::Debug for PendingBrowserAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingBrowserAuthorization")
            .field("client_id", &self.client_id)
            .field("verifier", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("prompt", &self.prompt)
            .field("authorization_code", &self.authorization_code.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BrowserPoll {
    Pending,
    Authorized(YouTubeCredential),
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
pub struct YouTubeCredential {
    access_token: String,
    refresh_token: String,
    token_type: String,
    scope: String,
    expires_at_ms: u64,
}

impl YouTubeCredential {
    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
    #[must_use]
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }
    #[must_use]
    pub const fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
}

impl fmt::Debug for YouTubeCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("YouTubeCredential")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("scope", &self.scope)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

impl Drop for YouTubeCredential {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct YouTubeIdentity {
    pub channel_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct YouTubeBroadcast {
    pub video_id: String,
    pub title: String,
    pub scheduled_start_time: Option<String>,
    pub actual_start_time: Option<String>,
}

#[derive(Clone)]
pub struct YouTubeOAuthClient {
    client: Client,
    auth_endpoint: Url,
    token_endpoint: Url,
    revoke_endpoint: Url,
    api_endpoint: Url,
}

impl YouTubeOAuthClient {
    pub fn new() -> Result<Self, YouTubeError> {
        Self::with_endpoints(AUTH_ENDPOINT, TOKEN_ENDPOINT, REVOKE_ENDPOINT, API_ENDPOINT)
    }

    #[doc(hidden)]
    pub fn with_endpoints(
        auth: &str,
        token: &str,
        revoke: &str,
        api: &str,
    ) -> Result<Self, YouTubeError> {
        Ok(Self {
            client: secure_client()?,
            auth_endpoint: parse_trusted_endpoint(auth)?,
            token_endpoint: parse_trusted_endpoint(token)?,
            revoke_endpoint: parse_trusted_endpoint(revoke)?,
            api_endpoint: parse_trusted_endpoint(api)?,
        })
    }

    pub async fn begin_authorization(
        &self,
        client_id: &str,
        now_ms: u64,
    ) -> Result<PendingBrowserAuthorization, YouTubeError> {
        validate_client_id(client_id)?;
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(YouTubeError::transport)?;
        let port = listener.local_addr().map_err(YouTubeError::transport)?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/oauth2/callback");
        let verifier = random_url_secret(32)?;
        let state = random_url_secret(32)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut authorization = self.auth_endpoint.clone();
        authorization
            .query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", READONLY_SCOPE)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
        let (sender, receiver) = oneshot::channel();
        tokio::spawn(receive_callback(listener, state, sender));
        Ok(PendingBrowserAuthorization {
            client_id: client_id.to_owned(),
            verifier: SecretString(verifier),
            redirect_uri,
            prompt: BrowserAuthorizationPrompt {
                authorization_uri: authorization.into(),
                expires_at_ms: now_ms.saturating_add(AUTH_LIFETIME.as_millis() as u64),
            },
            callback: Some(receiver),
            authorization_code: None,
        })
    }

    pub async fn poll_authorization(
        &self,
        pending: &mut PendingBrowserAuthorization,
        now_ms: u64,
    ) -> Result<BrowserPoll, YouTubeError> {
        if pending.is_expired_at(now_ms) {
            return Err(YouTubeError::Expired);
        }
        if pending.authorization_code.is_none() {
            let Some(receiver) = pending.callback.as_mut() else {
                return Err(YouTubeError::InvalidResponse);
            };
            match receiver.try_recv() {
                Ok(result) => {
                    pending.authorization_code = Some(SecretString(result?));
                    pending.callback = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => return Ok(BrowserPoll::Pending),
                Err(oneshot::error::TryRecvError::Closed) => return Err(YouTubeError::Expired),
            }
        }
        let code = pending.authorization_code.as_ref().ok_or(YouTubeError::InvalidResponse)?;
        let response = self
            .client
            .post(self.token_endpoint.clone())
            .form(&[
                ("client_id", pending.client_id.as_str()),
                ("code", code.expose()),
                ("code_verifier", pending.verifier.expose()),
                ("grant_type", "authorization_code"),
                ("redirect_uri", pending.redirect_uri.as_str()),
            ])
            .send()
            .await
            .map_err(YouTubeError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        let credential = parse_json::<TokenResponse>(&body)?.into_credential(now_ms, None)?;
        Ok(BrowserPoll::Authorized(credential))
    }

    pub async fn refresh(
        &self,
        client_id: &str,
        credential: &YouTubeCredential,
        now_ms: u64,
    ) -> Result<YouTubeCredential, YouTubeError> {
        validate_client_id(client_id)?;
        validate_token(credential.refresh_token())?;
        let response = self
            .client
            .post(self.token_endpoint.clone())
            .form(&[
                ("client_id", client_id),
                ("refresh_token", credential.refresh_token()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(YouTubeError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        parse_json::<TokenResponse>(&body)?
            .into_credential(now_ms, Some(credential.refresh_token()))
    }

    pub async fn identity(&self, access_token: &str) -> Result<YouTubeIdentity, YouTubeError> {
        let mut url =
            self.api_endpoint.join("channels").map_err(|_| YouTubeError::InvalidResponse)?;
        url.query_pairs_mut().append_pair("part", "id,snippet").append_pair("mine", "true");
        let payload: ChannelList = self.get_json(url, access_token).await?;
        let channel = payload.items.into_iter().next().ok_or(YouTubeError::InvalidResponse)?;
        validate_identifier(&channel.id)?;
        if channel.snippet.title.is_empty() || channel.snippet.title.chars().any(char::is_control) {
            return Err(YouTubeError::InvalidResponse);
        }
        Ok(YouTubeIdentity { channel_id: channel.id, display_name: channel.snippet.title })
    }

    pub async fn active_broadcasts(
        &self,
        access_token: &str,
        expected_channel_id: &str,
    ) -> Result<Vec<YouTubeBroadcast>, YouTubeError> {
        validate_identifier(expected_channel_id)?;
        let mut url =
            self.api_endpoint.join("liveBroadcasts").map_err(|_| YouTubeError::InvalidResponse)?;
        url.query_pairs_mut()
            .append_pair("part", "id,snippet,status")
            .append_pair("broadcastStatus", "active")
            .append_pair("broadcastType", "all")
            .append_pair("maxResults", "50");
        let payload: BroadcastList = self.get_json(url, access_token).await?;
        let mut broadcasts = Vec::new();
        for item in payload.items {
            if item.snippet.channel_id != expected_channel_id
                || item.status.life_cycle_status != "live"
            {
                continue;
            }
            validate_video_id(&item.id)?;
            validate_display_text(&item.snippet.title)?;
            broadcasts.push(YouTubeBroadcast {
                video_id: item.id,
                title: item.snippet.title,
                scheduled_start_time: clean_optional_text(item.snippet.scheduled_start_time)?,
                actual_start_time: clean_optional_text(item.snippet.actual_start_time)?,
            });
        }
        broadcasts.sort_by(|left, right| {
            right
                .actual_start_time
                .cmp(&left.actual_start_time)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.video_id.cmp(&right.video_id))
        });
        Ok(broadcasts)
    }

    pub async fn revoke(&self, token: &str) -> Result<(), YouTubeError> {
        validate_token(token)?;
        let response = self
            .client
            .post(self.revoke_endpoint.clone())
            .form(&[("token", token)])
            .send()
            .await
            .map_err(YouTubeError::transport)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(YouTubeError::Api {
                status: response.status().as_u16(),
                message: "revocation refused".to_owned(),
            })
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: Url,
        token: &str,
    ) -> Result<T, YouTubeError> {
        validate_token(token)?;
        let response = self
            .client
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(YouTubeError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        parse_json(&body)
    }
}

#[derive(Clone, Debug)]
pub struct YouTubeLiveConfig {
    pub source_id: String,
    pub video_id: String,
    pub next_source_sequence: u64,
    pub resume_checkpoint: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct YouTubeResumeState {
    chat_id: String,
    page_token: String,
    baseline_complete: bool,
}

#[derive(Clone)]
pub struct YouTubeLiveChatClient {
    client: Client,
    api_endpoint: Url,
    grpc_endpoint: String,
    resume: Arc<Mutex<HashMap<(String, String), YouTubeResumeState>>>,
}

impl YouTubeLiveChatClient {
    pub fn new() -> Result<Self, YouTubeError> {
        Self::with_endpoints(API_ENDPOINT, GRPC_ENDPOINT)
    }

    #[doc(hidden)]
    pub fn with_endpoint(endpoint: &str) -> Result<Self, YouTubeError> {
        Self::with_endpoints(endpoint, endpoint.trim_end_matches('/'))
    }

    #[doc(hidden)]
    pub fn with_endpoints(api_endpoint: &str, grpc_endpoint: &str) -> Result<Self, YouTubeError> {
        let grpc = parse_trusted_endpoint(grpc_endpoint)?;
        Ok(Self {
            client: secure_client()?,
            api_endpoint: parse_trusted_endpoint(api_endpoint)?,
            grpc_endpoint: grpc.as_str().trim_end_matches('/').to_owned(),
            resume: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn test_video(
        &self,
        video_id: &str,
        access_token: &str,
    ) -> Result<String, YouTubeError> {
        self.live_chat_id(video_id, access_token).await
    }

    pub async fn run(
        &self,
        config: YouTubeLiveConfig,
        credential: &YouTubeCredential,
        output: mpsc::Sender<SourceAdapterEvent>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), YouTubeError> {
        validate_identifier(&config.source_id)?;
        validate_video_id(&config.video_id)?;
        emit_state(&output, &config.source_id, SourceRuntimeState::Connecting, None)?;
        let chat_id = self.live_chat_id(&config.video_id, credential.access_token()).await?;
        let resume_key = (config.source_id.clone(), chat_id.clone());
        let checkpoint = config
            .resume_checkpoint
            .as_deref()
            .and_then(parse_resume_checkpoint)
            .filter(|state| state.chat_id == chat_id)
            .unwrap_or_else(|| YouTubeResumeState {
                chat_id: chat_id.clone(),
                ..Default::default()
            });
        let mut resume =
            self.resume.lock().await.entry(resume_key.clone()).or_insert(checkpoint).clone();
        let mut next_sequence = config.next_source_sequence;
        let mut recent_message_ids = VecDeque::<String>::new();
        let mut resume_backoff = Duration::from_secs(1);
        loop {
            let channel = self.grpc_channel().await?;
            let mut client = stream_api::v3_data_live_chat_message_service_client::V3DataLiveChatMessageServiceClient::new(channel);
            let mut request = Request::new(stream_api::LiveChatMessageListRequest {
                live_chat_id: chat_id.clone(),
                hl: String::new(),
                profile_image_size: 0,
                page_token: resume.page_token.clone(),
                part: vec!["snippet".to_owned()],
            });
            let mut authorization =
                MetadataValue::try_from(format!("Bearer {}", credential.access_token()))
                    .map_err(|_| YouTubeError::InvalidResponse)?;
            authorization.set_sensitive(true);
            request.metadata_mut().insert("authorization", authorization);
            let mut stream = match client.stream_list(request).await {
                Ok(response) => response.into_inner(),
                Err(status)
                    if status.code() == Code::InvalidArgument && !resume.page_token.is_empty() =>
                {
                    resume = YouTubeResumeState { chat_id: chat_id.clone(), ..Default::default() };
                    self.queue_checkpoint(&config.source_id, &resume_key, &resume, &output).await?;
                    emit_state(
                        &output,
                        &config.source_id,
                        SourceRuntimeState::Backoff,
                        Some("cursor_rejected_gap_rebaseline".to_owned()),
                    )?;
                    continue;
                }
                Err(status) => return Err(grpc_error(status)),
            };
            emit_state(&output, &config.source_id, SourceRuntimeState::Connected, None)?;
            loop {
                let batch = tokio::select! {
                    message = stream.message() => match message {
                        Ok(value) => value,
                        Err(status) if status.code() == Code::InvalidArgument && !resume.page_token.is_empty() => {
                            resume = YouTubeResumeState { chat_id: chat_id.clone(), ..Default::default() };
                            self.queue_checkpoint(&config.source_id, &resume_key, &resume, &output).await?;
                            emit_state(
                                &output,
                                &config.source_id,
                                SourceRuntimeState::Backoff,
                                Some("cursor_rejected_gap_rebaseline".to_owned()),
                            )?;
                            break;
                        }
                        Err(status) => return Err(grpc_error(status)),
                    },
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            emit_state(&output, &config.source_id, SourceRuntimeState::Paused, None)?;
                            return Ok(());
                        }
                        continue;
                    }
                };
                let Some(batch) = batch else {
                    emit_state(
                        &output,
                        &config.source_id,
                        SourceRuntimeState::Backoff,
                        Some("stream ended; resuming from provider cursor".to_owned()),
                    )?;
                    tokio::select! {
                        _ = tokio::time::sleep(resume_backoff) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                emit_state(&output, &config.source_id, SourceRuntimeState::Paused, None)?;
                                return Ok(());
                            }
                        }
                    }
                    resume_backoff = (resume_backoff * 2).min(Duration::from_secs(60));
                    break;
                };
                resume_backoff = Duration::from_secs(1);
                if batch.offline_at.is_some() {
                    emit_state(
                        &output,
                        &config.source_id,
                        SourceRuntimeState::Paused,
                        Some("live_ended".to_owned()),
                    )?;
                    return Err(YouTubeError::LiveEnded);
                }
                let next_page_token = batch.next_page_token;
                let baseline = !resume.baseline_complete;
                for item in batch.items {
                    validate_provider_identifier(&item.id)?;
                    if recent_message_ids.iter().any(|message_id| message_id == &item.id) {
                        continue;
                    }
                    recent_message_ids.push_back(item.id.clone());
                    if recent_message_ids.len() > MAX_RECENT_MESSAGE_IDS {
                        recent_message_ids.pop_front();
                    }
                    if baseline {
                        continue;
                    }
                    let Some(snippet) = item.snippet else {
                        continue;
                    };
                    if snippet.r#type != stream_api::LiveChatMessageType::TextMessageEvent as i32 {
                        continue;
                    }
                    let Some(details) = snippet.text_message_details else {
                        continue;
                    };
                    if details.message_text.is_empty()
                        || details.message_text.chars().count() > MAX_SUBMISSION_CHARS
                        || details.message_text.contains('\0')
                    {
                        continue;
                    }
                    let participant_id = snippet.author_channel_id;
                    validate_provider_identifier(&participant_id)?;
                    let message_id = scoped_message_id(&config.source_id, &item.id);
                    output
                        .try_send(SourceAdapterEvent::Message(SourceMessage {
                            source_id: config.source_id.clone(),
                            message_id,
                            participant_id: format!("youtube:{participant_id}"),
                            source_sequence: next_sequence,
                            text: details.message_text,
                            // The initial provider cursor is the ordering boundary. Provider
                            // timestamps are metadata and are not trusted as a local clock.
                            occurred_at_ms: now_ms(),
                        }))
                        .map_err(|_| YouTubeError::Backpressure)?;
                    next_sequence =
                        next_sequence.checked_add(1).ok_or(YouTubeError::InvalidResponse)?;
                }
                resume.page_token = next_page_token;
                resume.baseline_complete = true;
                self.queue_checkpoint(&config.source_id, &resume_key, &resume, &output).await?;
            }
        }
    }

    async fn queue_checkpoint(
        &self,
        source_id: &str,
        resume_key: &(String, String),
        resume: &YouTubeResumeState,
        output: &mpsc::Sender<SourceAdapterEvent>,
    ) -> Result<(), YouTubeError> {
        let cursor = serde_json::to_string(resume).map_err(|_| YouTubeError::InvalidResponse)?;
        output
            .try_send(SourceAdapterEvent::Checkpoint { source_id: source_id.to_owned(), cursor })
            .map_err(|_| YouTubeError::Backpressure)?;
        self.resume.lock().await.insert(resume_key.clone(), resume.clone());
        Ok(())
    }

    async fn grpc_channel(&self) -> Result<GrpcChannel, YouTubeError> {
        let mut endpoint = Endpoint::from_shared(self.grpc_endpoint.clone())
            .map_err(|_| YouTubeError::InvalidConfig("gRPC endpoint is invalid"))?
            .connect_timeout(Duration::from_secs(10));
        if self.grpc_endpoint.starts_with("https://") {
            endpoint = endpoint
                .tls_config(
                    ClientTlsConfig::new()
                        .with_webpki_roots()
                        .domain_name("youtube.googleapis.com"),
                )
                .map_err(YouTubeError::transport)?;
        }
        endpoint.connect().await.map_err(YouTubeError::transport)
    }

    async fn live_chat_id(&self, video_id: &str, token: &str) -> Result<String, YouTubeError> {
        validate_video_id(video_id)?;
        validate_token(token)?;
        let mut url =
            self.api_endpoint.join("videos").map_err(|_| YouTubeError::InvalidResponse)?;
        url.query_pairs_mut()
            .append_pair("part", "liveStreamingDetails")
            .append_pair("id", video_id);
        let response = self
            .client
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(YouTubeError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        let list: VideoList = parse_json(&body)?;
        let chat_id = list
            .items
            .into_iter()
            .next()
            .and_then(|video| video.live_streaming_details.active_live_chat_id)
            .ok_or(YouTubeError::NoActiveLiveChat)?;
        validate_identifier(&chat_id)?;
        Ok(chat_id)
    }
}

pub fn validate_youtube_client_id(value: &str) -> Result<(), YouTubeError> {
    validate_client_id(value)
}

pub fn validate_video_id(value: &str) -> Result<(), YouTubeError> {
    if value.len() != 11
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(YouTubeError::InvalidConfig("video ID must be the 11-character YouTube ID"));
    }
    Ok(())
}

pub fn store_credential(
    vault: &dyn CredentialVault,
    credential_id: &str,
    credential: &YouTubeCredential,
) -> Result<(), YouTubeError> {
    let mut bytes = serde_json::to_vec(credential).map_err(|_| YouTubeError::InvalidResponse)?;
    let result = vault.store(credential_id, &bytes).map_err(YouTubeError::Vault);
    bytes.zeroize();
    result
}

pub fn load_credential(
    vault: &dyn CredentialVault,
    credential_id: &str,
) -> Result<YouTubeCredential, YouTubeError> {
    let secret = vault.load(credential_id).map_err(YouTubeError::Vault)?;
    let credential: YouTubeCredential =
        serde_json::from_slice(secret.expose()).map_err(|_| YouTubeError::InvalidResponse)?;
    credential.validate()?;
    Ok(credential)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YouTubeError {
    InvalidConfig(&'static str),
    InvalidResponse,
    ResponseTooLarge,
    Expired,
    AccessDenied,
    NoActiveLiveChat,
    LiveEnded,
    QuotaExhausted,
    Api { status: u16, message: String },
    Transport(String),
    Vault(VaultError),
    Backpressure,
}

impl YouTubeError {
    fn transport(error: impl fmt::Display) -> Self {
        Self::Transport(error.to_string())
    }
}

impl fmt::Display for YouTubeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(reason) => write!(f, "invalid YouTube configuration: {reason}"),
            Self::InvalidResponse => write!(f, "YouTube returned an invalid response"),
            Self::ResponseTooLarge => write!(f, "YouTube response exceeds the size limit"),
            Self::Expired => write!(f, "YouTube authorization expired"),
            Self::AccessDenied => write!(f, "YouTube authorization was denied"),
            Self::NoActiveLiveChat => write!(f, "the video has no active YouTube live chat"),
            Self::LiveEnded => write!(f, "the YouTube live chat has ended"),
            Self::QuotaExhausted => write!(f, "the YouTube API quota is exhausted"),
            Self::Api { status, message } => write!(f, "YouTube API error {status}: {message}"),
            Self::Transport(message) => write!(f, "YouTube transport error: {message}"),
            Self::Vault(error) => write!(f, "YouTube credential error: {error}"),
            Self::Backpressure => write!(f, "YouTube source output is backpressured"),
        }
    }
}
impl std::error::Error for YouTubeError {}

struct SecretString(String);
impl SecretString {
    fn expose(&self) -> &str {
        &self.0
    }
}
impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    token_type: String,
    #[serde(default)]
    scope: String,
    expires_in: u64,
}
impl TokenResponse {
    fn into_credential(
        self,
        now_ms: u64,
        existing_refresh: Option<&str>,
    ) -> Result<YouTubeCredential, YouTubeError> {
        let refresh_token = self
            .refresh_token
            .or_else(|| existing_refresh.map(str::to_owned))
            .ok_or(YouTubeError::InvalidResponse)?;
        validate_token(&self.access_token)?;
        validate_token(&refresh_token)?;
        if !self.token_type.eq_ignore_ascii_case("bearer")
            || self.expires_in == 0
            || (!self.scope.is_empty()
                && !self.scope.split_whitespace().any(|scope| scope == READONLY_SCOPE))
        {
            return Err(YouTubeError::InvalidResponse);
        }
        Ok(YouTubeCredential {
            access_token: self.access_token,
            refresh_token,
            token_type: self.token_type,
            scope: self.scope,
            expires_at_ms: now_ms.saturating_add(self.expires_in.saturating_mul(1_000)),
        })
    }
}
impl YouTubeCredential {
    fn validate(&self) -> Result<(), YouTubeError> {
        validate_token(&self.access_token)?;
        validate_token(&self.refresh_token)?;
        if !self.token_type.eq_ignore_ascii_case("bearer") || self.expires_at_ms == 0 {
            return Err(YouTubeError::InvalidResponse);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct ChannelList {
    #[serde(default)]
    items: Vec<Channel>,
}
#[derive(Deserialize)]
struct Channel {
    id: String,
    snippet: ChannelSnippet,
}
#[derive(Deserialize)]
struct ChannelSnippet {
    title: String,
}
#[derive(Deserialize)]
struct BroadcastList {
    #[serde(default)]
    items: Vec<Broadcast>,
}
#[derive(Deserialize)]
struct Broadcast {
    id: String,
    snippet: BroadcastSnippet,
    status: BroadcastStatus,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BroadcastSnippet {
    title: String,
    channel_id: String,
    #[serde(default)]
    scheduled_start_time: Option<String>,
    #[serde(default)]
    actual_start_time: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BroadcastStatus {
    life_cycle_status: String,
}
#[derive(Deserialize)]
struct VideoList {
    #[serde(default)]
    items: Vec<Video>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Video {
    live_streaming_details: LiveStreamingDetails,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveStreamingDetails {
    #[serde(default)]
    active_live_chat_id: Option<String>,
}
async fn receive_callback(
    listener: tokio::net::TcpListener,
    expected_state: String,
    sender: oneshot::Sender<Result<String, YouTubeError>>,
) {
    let result = tokio::time::timeout(AUTH_LIFETIME, async {
        let (mut stream, address) = listener.accept().await.map_err(YouTubeError::transport)?;
        if !address.ip().is_loopback() { return Err(YouTubeError::AccessDenied); }
        let mut bytes = vec![0_u8; MAX_CALLBACK_BYTES];
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let count = stream.read(&mut bytes).await.map_err(YouTubeError::transport)?;
        let request = std::str::from_utf8(&bytes[..count]).map_err(|_| YouTubeError::InvalidResponse)?;
        let target = request.lines().next().and_then(|line| line.split_whitespace().nth(1)).ok_or(YouTubeError::InvalidResponse)?;
        let url = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| YouTubeError::InvalidResponse)?;
        if url.path() != "/oauth2/callback" { return Err(YouTubeError::AccessDenied); }
        let mut code = None; let mut state = None; let mut denied = false;
        for (key, value) in url.query_pairs() {
            match key.as_ref() { "code" => code = Some(value.into_owned()), "state" => state = Some(value.into_owned()), "error" => denied = true, _ => {} }
        }
        let outcome = if denied || state.as_deref() != Some(expected_state.as_str()) {
            Err(YouTubeError::AccessDenied)
        } else {
            code.filter(|value| !value.is_empty() && value.len() <= MAX_TOKEN_CHARS)
                .ok_or(YouTubeError::InvalidResponse)
        };
        let success = outcome.is_ok();
        let body = if success { "Authorization received. You can return to Semantic Engine." } else { "Authorization refused. Return to Semantic Engine." };
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = stream.write_all(response.as_bytes()).await;
        outcome
    }).await.unwrap_or(Err(YouTubeError::Expired));
    let _ = sender.send(result);
}

fn secure_client() -> Result<Client, YouTubeError> {
    Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent("SemanticEngine/0.1")
        .build()
        .map_err(YouTubeError::transport)
}
fn parse_trusted_endpoint(value: &str) -> Result<Url, YouTubeError> {
    let url = Url::parse(value).map_err(|_| YouTubeError::InvalidConfig("endpoint is invalid"))?;
    let loopback = url.host_str().is_some_and(|host| host == "127.0.0.1" || host == "localhost");
    if url.host_str().is_none()
        || (url.scheme() != "https" && !(cfg!(test) && url.scheme() == "http" && loopback))
    {
        return Err(YouTubeError::InvalidConfig("endpoint must use HTTPS"));
    }
    Ok(url)
}
fn validate_client_id(value: &str) -> Result<(), YouTubeError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
        || !value.ends_with(".apps.googleusercontent.com")
    {
        return Err(YouTubeError::InvalidConfig(
            "client ID must be a Google Desktop OAuth client ID",
        ));
    }
    Ok(())
}
fn validate_identifier(value: &str) -> Result<(), YouTubeError> {
    if value.is_empty()
        || value.chars().count() > MAX_IDENTIFIER_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(YouTubeError::InvalidResponse);
    }
    Ok(())
}
fn validate_display_text(value: &str) -> Result<(), YouTubeError> {
    if value.is_empty() || value.chars().count() > 256 || value.chars().any(char::is_control) {
        return Err(YouTubeError::InvalidResponse);
    }
    Ok(())
}
fn clean_optional_text(value: Option<String>) -> Result<Option<String>, YouTubeError> {
    match value {
        Some(value) => {
            validate_display_text(&value)?;
            Ok(Some(value))
        }
        None => Ok(None),
    }
}
fn validate_provider_identifier(value: &str) -> Result<(), YouTubeError> {
    if value.is_empty()
        || value.chars().count() > MAX_IDENTIFIER_CHARS.saturating_sub("youtube:".len())
        || value.chars().any(char::is_control)
    {
        return Err(YouTubeError::InvalidResponse);
    }
    Ok(())
}
fn scoped_message_id(source_id: &str, provider_message_id: &str) -> String {
    let source_digest = Sha256::digest(source_id.as_bytes());
    let message_digest = Sha256::digest(provider_message_id.as_bytes());
    format!(
        "youtube:{}:{}",
        URL_SAFE_NO_PAD.encode(&source_digest[..12]),
        URL_SAFE_NO_PAD.encode(&message_digest[..24])
    )
}
fn parse_resume_checkpoint(value: &str) -> Option<YouTubeResumeState> {
    if value.len() > MAX_RESPONSE_BYTES || value.chars().any(char::is_control) {
        return None;
    }
    let state = serde_json::from_str::<YouTubeResumeState>(value).ok()?;
    if validate_identifier(&state.chat_id).is_err()
        || state.page_token.len() > MAX_TOKEN_CHARS
        || state.page_token.chars().any(char::is_control)
    {
        return None;
    }
    Some(state)
}
fn validate_token(value: &str) -> Result<(), YouTubeError> {
    if value.is_empty()
        || value.chars().count() > MAX_TOKEN_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(YouTubeError::InvalidResponse);
    }
    Ok(())
}
fn random_url_secret(bytes: usize) -> Result<String, YouTubeError> {
    let mut value = vec![0_u8; bytes];
    getrandom::fill(&mut value)
        .map_err(|_| YouTubeError::InvalidConfig("OS randomness is unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(value))
}
async fn read_limited(response: reqwest::Response) -> Result<Vec<u8>, YouTubeError> {
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(YouTubeError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(YouTubeError::transport)?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(YouTubeError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
fn parse_json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, YouTubeError> {
    serde_json::from_slice(body).map_err(|_| YouTubeError::InvalidResponse)
}
fn api_error(status: StatusCode, body: &[u8]) -> YouTubeError {
    #[derive(Deserialize)]
    struct Envelope {
        error: ApiMessage,
    }
    #[derive(Deserialize)]
    struct ApiMessage {
        message: String,
    }
    let message = serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|value| value.error.message)
        .filter(|value| value.len() <= 256 && !value.chars().any(char::is_control))
        .unwrap_or_else(|| "request refused".to_owned());
    YouTubeError::Api { status: status.as_u16(), message }
}
fn grpc_error(status: tonic::Status) -> YouTubeError {
    match status.code() {
        Code::Unauthenticated => {
            YouTubeError::Api { status: 401, message: "authorization required".to_owned() }
        }
        Code::ResourceExhausted => YouTubeError::QuotaExhausted,
        Code::PermissionDenied => {
            YouTubeError::Api { status: 403, message: "permission denied".to_owned() }
        }
        Code::NotFound | Code::FailedPrecondition => YouTubeError::LiveEnded,
        Code::InvalidArgument => YouTubeError::InvalidResponse,
        _ => YouTubeError::Transport(format!("gRPC stream error: {}", status.code())),
    }
}
fn emit_state(
    output: &mpsc::Sender<SourceAdapterEvent>,
    source_id: &str,
    state: SourceRuntimeState,
    detail: Option<String>,
) -> Result<(), YouTubeError> {
    output
        .try_send(SourceAdapterEvent::StateChanged {
            source_id: source_id.to_owned(),
            state,
            detail,
        })
        .map_err(|_| YouTubeError::Backpressure)
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;

    use axum::{Json, Router, extract::Query, http::HeaderMap, routing::get};
    use tokio_stream::{Stream, wrappers::TcpListenerStream};
    use tonic::{Response, Status};

    use super::*;

    #[test]
    fn youtube_ids_are_strict_and_credentials_are_redacted() {
        assert!(validate_youtube_client_id("123-abc.apps.googleusercontent.com").is_ok());
        assert!(validate_youtube_client_id("not-google").is_err());
        assert!(validate_video_id("dQw4w9WgXcQ").is_ok());
        assert!(validate_video_id("https://youtube.invalid").is_err());
        let credential = YouTubeCredential {
            access_token: "access-secret".into(),
            refresh_token: "refresh-secret".into(),
            token_type: "Bearer".into(),
            scope: READONLY_SCOPE.into(),
            expires_at_ms: 99,
        };
        let debug = format!("{credential:?}");
        assert!(!debug.contains("secret"));
    }

    #[tokio::test]
    async fn browser_authorization_uses_pkce_state_and_loopback() {
        let client = YouTubeOAuthClient::with_endpoints(
            "https://accounts.google.test/auth",
            "https://accounts.google.test/token",
            "https://accounts.google.test/revoke",
            "https://youtube.googleapis.test/",
        )
        .unwrap();
        let pending =
            client.begin_authorization("123.apps.googleusercontent.com", 1_000).await.unwrap();
        let url = Url::parse(&pending.prompt().authorization_uri).unwrap();
        let pairs = url.query_pairs().collect::<std::collections::HashMap<_, _>>();
        assert_eq!(pairs.get("code_challenge_method").map(|v| v.as_ref()), Some("S256"));
        assert_eq!(pairs.get("scope").map(|v| v.as_ref()), Some(READONLY_SCOPE));
        assert!(pairs.get("state").is_some_and(|value| value.len() >= 32));
        assert!(
            pairs.get("redirect_uri").is_some_and(|value| value.starts_with("http://127.0.0.1:"))
        );
        assert!(!format!("{pending:?}").contains(pending.verifier.expose()));
    }

    #[tokio::test]
    async fn active_broadcast_discovery_is_scoped_and_filters_untrusted_results() {
        let app = Router::new().route(
            "/liveBroadcasts",
            get(|headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
                assert_eq!(
                    headers.get("authorization").and_then(|value| value.to_str().ok()),
                    Some("Bearer access-token")
                );
                assert_eq!(query.get("broadcastStatus").map(String::as_str), Some("active"));
                assert_eq!(query.get("broadcastType").map(String::as_str), Some("all"));
                Json(serde_json::json!({"items": [
                    {
                        "id": "dQw4w9WgXcQ",
                        "snippet": {
                            "title": "Guess the game",
                            "channelId": "channel-owner",
                            "scheduledStartTime": "2026-08-01T10:00:00Z",
                            "actualStartTime": "2026-08-01T10:02:00Z"
                        },
                        "status": {"lifeCycleStatus": "live"}
                    },
                    {
                        "id": "abcdefghijk",
                        "snippet": {"title": "Other channel", "channelId": "other"},
                        "status": {"lifeCycleStatus": "live"}
                    },
                    {
                        "id": "lmnopqrstuv",
                        "snippet": {"title": "Not live", "channelId": "channel-owner"},
                        "status": {"lifeCycleStatus": "complete"}
                    }
                ]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = YouTubeOAuthClient::with_endpoints(
            "https://accounts.google.test/auth",
            "https://accounts.google.test/token",
            "https://accounts.google.test/revoke",
            &format!("http://{address}/"),
        )
        .unwrap();

        let broadcasts = client.active_broadcasts("access-token", "channel-owner").await.unwrap();

        assert_eq!(broadcasts.len(), 1);
        assert_eq!(broadcasts[0].video_id, "dQw4w9WgXcQ");
        assert_eq!(broadcasts[0].title, "Guess the game");
        server.abort();
    }

    #[test]
    fn grpc_failures_and_message_ids_keep_product_semantics() {
        assert_eq!(grpc_error(Status::resource_exhausted("quota")), YouTubeError::QuotaExhausted);
        assert_eq!(grpc_error(Status::not_found("ended")), YouTubeError::LiveEnded);
        assert_ne!(
            scoped_message_id("youtube-left", "provider-message"),
            scoped_message_id("youtube-right", "provider-message")
        );

        let checkpoint = YouTubeResumeState {
            chat_id: "chat-42".to_owned(),
            page_token: "cursor-1".to_owned(),
            baseline_complete: true,
        };
        let encoded = serde_json::to_string(&checkpoint).unwrap();
        assert_eq!(parse_resume_checkpoint(&encoded).unwrap().page_token, "cursor-1");
    }

    #[tokio::test]
    async fn live_chat_baselines_history_then_emits_and_deduplicates_text() {
        let app = Router::new().route(
            "/videos",
            get(|| async {
                Json(serde_json::json!({"items": [{
                    "liveStreamingDetails": {"activeLiveChatId": "chat-42"}
                }]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        struct MockStream;
        #[tonic::async_trait]
        impl stream_api::v3_data_live_chat_message_service_server::V3DataLiveChatMessageService
            for MockStream
        {
            type StreamListStream = Pin<
                Box<
                    dyn Stream<Item = Result<stream_api::LiveChatMessageListResponse, Status>>
                        + Send,
                >,
            >;

            async fn stream_list(
                &self,
                request: Request<stream_api::LiveChatMessageListRequest>,
            ) -> Result<Response<Self::StreamListStream>, Status> {
                if request.metadata().get("authorization").and_then(|value| value.to_str().ok())
                    != Some("Bearer access-token")
                {
                    return Err(Status::unauthenticated("missing bearer"));
                }
                let message =
                    |id: &str, participant: &str, text: &str| stream_api::LiveChatMessage {
                        id: id.to_owned(),
                        snippet: Some(stream_api::LiveChatMessageSnippet {
                            r#type: stream_api::LiveChatMessageType::TextMessageEvent as i32,
                            author_channel_id: participant.to_owned(),
                            text_message_details: Some(stream_api::LiveChatTextMessageDetails {
                                message_text: text.to_owned(),
                            }),
                            ..Default::default()
                        }),
                    };
                let batches = vec![
                    Ok(stream_api::LiveChatMessageListResponse {
                        next_page_token: "cursor-1".to_owned(),
                        items: vec![message("old-message", "old-viewer", "old answer")],
                        ..Default::default()
                    }),
                    Ok(stream_api::LiveChatMessageListResponse {
                        next_page_token: "cursor-2".to_owned(),
                        items: vec![message("new-message", "viewer-42", "Elden Ring")],
                        ..Default::default()
                    }),
                ];
                Ok(Response::new(Box::pin(tokio_stream::iter(batches))))
            }
        }
        let grpc_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let grpc_address = grpc_listener.local_addr().unwrap();
        let grpc_server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(
                    stream_api::v3_data_live_chat_message_service_server::V3DataLiveChatMessageServiceServer::new(MockStream),
                )
                .serve_with_incoming(TcpListenerStream::new(grpc_listener))
                .await
                .unwrap();
        });
        let client = YouTubeLiveChatClient::with_endpoints(
            &format!("http://{address}/"),
            &format!("http://{grpc_address}"),
        )
        .unwrap();
        let credential = YouTubeCredential {
            access_token: "access-token".into(),
            refresh_token: "refresh-token".into(),
            token_type: "Bearer".into(),
            scope: READONLY_SCOPE.into(),
            expires_at_ms: u64::MAX,
        };
        let (output, mut events) = mpsc::channel(8);
        let (shutdown, receiver) = watch::channel(false);
        let task = tokio::spawn(async move {
            client
                .run(
                    YouTubeLiveConfig {
                        source_id: "youtube-main".into(),
                        video_id: "dQw4w9WgXcQ".into(),
                        next_source_sequence: 7,
                        resume_checkpoint: None,
                    },
                    &credential,
                    output,
                    receiver,
                )
                .await
        });
        let delivered = loop {
            let event =
                tokio::time::timeout(Duration::from_secs(3), events.recv()).await.unwrap().unwrap();
            if let SourceAdapterEvent::Message(message) = event {
                break message;
            }
        };
        assert_eq!(delivered.message_id, scoped_message_id("youtube-main", "new-message"));
        assert_eq!(delivered.participant_id, "youtube:viewer-42");
        assert_eq!(delivered.source_sequence, 7);
        assert_eq!(delivered.text, "Elden Ring");
        shutdown.send(true).unwrap();
        task.await.unwrap().unwrap();
        server.abort();
        grpc_server.abort();
    }
}
