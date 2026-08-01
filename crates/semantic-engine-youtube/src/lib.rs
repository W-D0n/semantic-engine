use std::{collections::VecDeque, fmt, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use semantic_engine_core::{MAX_IDENTIFIER_CHARS, MAX_SUBMISSION_CHARS};
use semantic_engine_credential_vault::{CredentialVault, VaultError};
use semantic_engine_source::{SourceAdapterEvent, SourceMessage, SourceRuntimeState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot, watch};
use zeroize::Zeroize;

pub const YOUTUBE_ADAPTER_ID: &str = "youtube-live-chat";
pub const READONLY_SCOPE: &str = "https://www.googleapis.com/auth/youtube.readonly";

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
const API_ENDPOINT: &str = "https://www.googleapis.com/youtube/v3/";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_TOKEN_CHARS: usize = 8_192;
const MAX_CALLBACK_BYTES: usize = 8 * 1024;
const AUTH_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MIN_POLL_MS: u64 = 1_000;
const MAX_RECENT_MESSAGE_IDS: usize = 4_096;

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
}

#[derive(Clone)]
pub struct YouTubeLiveChatClient {
    client: Client,
    api_endpoint: Url,
}

impl YouTubeLiveChatClient {
    pub fn new() -> Result<Self, YouTubeError> {
        Self::with_endpoint(API_ENDPOINT)
    }

    #[doc(hidden)]
    pub fn with_endpoint(endpoint: &str) -> Result<Self, YouTubeError> {
        Ok(Self { client: secure_client()?, api_endpoint: parse_trusted_endpoint(endpoint)? })
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
        emit_state(&output, &config.source_id, SourceRuntimeState::Connected, None)?;
        let mut page_token: Option<String> = None;
        let mut next_sequence = config.next_source_sequence;
        let mut recent_message_ids = VecDeque::<String>::new();
        loop {
            let mut url = self
                .api_endpoint
                .join("liveChat/messages")
                .map_err(|_| YouTubeError::InvalidResponse)?;
            url.query_pairs_mut()
                .append_pair("liveChatId", &chat_id)
                .append_pair("part", "id,snippet,authorDetails")
                .append_pair("maxResults", "200");
            if let Some(token) = &page_token {
                url.query_pairs_mut().append_pair("pageToken", token);
            }
            let response = self
                .client
                .get(url)
                .bearer_auth(credential.access_token())
                .send()
                .await
                .map_err(YouTubeError::transport)?;
            let status = response.status();
            let body = read_limited(response).await?;
            if !status.is_success() {
                return Err(api_error(status, &body));
            }
            let batch: MessageList = parse_json(&body)?;
            let poll_ms = batch.polling_interval_millis.max(MIN_POLL_MS);
            let baseline = page_token.is_none();
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
                if item.snippet.kind != "textMessageEvent" {
                    continue;
                }
                let Some(details) = item.snippet.text_message_details else {
                    continue;
                };
                if details.message_text.is_empty()
                    || details.message_text.chars().count() > MAX_SUBMISSION_CHARS
                    || details.message_text.contains('\0')
                {
                    continue;
                }
                validate_provider_identifier(&item.author_details.channel_id)?;
                output
                    .try_send(SourceAdapterEvent::Message(SourceMessage {
                        source_id: config.source_id.clone(),
                        message_id: format!("youtube:{}", item.id),
                        participant_id: format!("youtube:{}", item.author_details.channel_id),
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
            page_token = Some(batch.next_page_token);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(poll_ms)) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        emit_state(&output, &config.source_id, SourceRuntimeState::Paused, None)?;
                        return Ok(());
                    }
                }
            }
        }
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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageList {
    next_page_token: String,
    polling_interval_millis: u64,
    #[serde(default)]
    items: Vec<LiveMessage>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveMessage {
    id: String,
    snippet: MessageSnippet,
    author_details: AuthorDetails,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageSnippet {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text_message_details: Option<TextDetails>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextDetails {
    message_text: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthorDetails {
    channel_id: String,
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
fn validate_provider_identifier(value: &str) -> Result<(), YouTubeError> {
    if value.is_empty()
        || value.chars().count() > MAX_IDENTIFIER_CHARS.saturating_sub("youtube:".len())
        || value.chars().any(char::is_control)
    {
        return Err(YouTubeError::InvalidResponse);
    }
    Ok(())
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
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, extract::State, routing::get};

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
    async fn live_chat_baselines_history_then_emits_and_deduplicates_text() {
        let polls = Arc::new(Mutex::new(0_u8));
        let app = Router::new()
            .route(
                "/videos",
                get(|| async {
                    Json(serde_json::json!({"items": [{
                        "liveStreamingDetails": {"activeLiveChatId": "chat-42"}
                    }]}))
                }),
            )
            .route(
                "/liveChat/messages",
                get(|State(polls): State<Arc<Mutex<u8>>>| async move {
                    let mut count = polls.lock().unwrap();
                    *count += 1;
                    let items = if *count == 1 {
                        serde_json::json!([{
                            "id": "old-message",
                            "snippet": {"type": "textMessageEvent", "publishedAt": "2026-01-01T00:00:00Z", "textMessageDetails": {"messageText": "old answer"}},
                            "authorDetails": {"channelId": "old-viewer"}
                        }])
                    } else {
                        serde_json::json!([{
                            "id": "new-message",
                            "snippet": {"type": "textMessageEvent", "publishedAt": "2026-01-01T00:00:01Z", "textMessageDetails": {"messageText": "Elden Ring"}},
                            "authorDetails": {"channelId": "viewer-42"}
                        }])
                    };
                    Json(serde_json::json!({
                        "nextPageToken": format!("page-{count}"),
                        "pollingIntervalMillis": 1,
                        "items": items
                    }))
                }),
            )
            .with_state(polls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = YouTubeLiveChatClient::with_endpoint(&format!("http://{address}/")).unwrap();
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
        assert_eq!(delivered.message_id, "youtube:new-message");
        assert_eq!(delivered.participant_id, "youtube:viewer-42");
        assert_eq!(delivered.source_sequence, 7);
        assert_eq!(delivered.text, "Elden Ring");
        shutdown.send(true).unwrap();
        task.await.unwrap().unwrap();
        server.abort();
    }
}
