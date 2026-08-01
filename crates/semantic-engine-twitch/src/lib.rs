use std::{fmt, time::Duration};

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use semantic_engine_credential_vault::{CredentialVault, VaultError};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

mod eventsub;

pub use eventsub::{EventSubConfig, TwitchEventSubClient};

pub const TWITCH_ADAPTER_ID: &str = "twitch-eventsub";
pub const CHAT_SCOPE: &str = "user:read:chat";

pub fn validate_twitch_client_id(client_id: &str) -> Result<(), TwitchError> {
    validate_client_id(client_id)
}

const DEVICE_ENDPOINT: &str = "https://id.twitch.tv/oauth2/device";
const TOKEN_ENDPOINT: &str = "https://id.twitch.tv/oauth2/token";
const VALIDATE_ENDPOINT: &str = "https://id.twitch.tv/oauth2/validate";
const REVOKE_ENDPOINT: &str = "https://id.twitch.tv/oauth2/revoke";
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_CLIENT_ID_CHARS: usize = 128;
const MAX_TOKEN_CHARS: usize = 2_048;
const MAX_DEVICE_CODE_CHARS: usize = 512;
const MAX_USER_CODE_CHARS: usize = 64;
const MIN_POLL_INTERVAL_SECONDS: u64 = 1;
const MAX_POLL_INTERVAL_SECONDS: u64 = 60;
const MAX_DEVICE_LIFETIME_SECONDS: u64 = 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceAuthorizationPrompt {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at_ms: u64,
    pub poll_interval_seconds: u64,
}

pub struct PendingDeviceAuthorization {
    client_id: String,
    device_code: SecretString,
    scopes: Vec<String>,
    prompt: DeviceAuthorizationPrompt,
}

impl PendingDeviceAuthorization {
    #[must_use]
    pub fn prompt(&self) -> &DeviceAuthorizationPrompt {
        &self.prompt
    }

    #[must_use]
    pub fn is_expired_at(&self, now_ms: u64) -> bool {
        now_ms >= self.prompt.expires_at_ms
    }
}

impl fmt::Debug for PendingDeviceAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingDeviceAuthorization")
            .field("client_id", &self.client_id)
            .field("device_code", &"[REDACTED]")
            .field("scopes", &self.scopes)
            .field("prompt", &self.prompt)
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    SlowDown,
    Authorized(TwitchCredential),
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
pub struct TwitchCredential {
    access_token: String,
    refresh_token: String,
    scopes: Vec<String>,
    token_type: String,
    expires_at_ms: u64,
}

impl TwitchCredential {
    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    #[must_use]
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    #[must_use]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    #[must_use]
    pub const fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
}

impl fmt::Debug for TwitchCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TwitchCredential")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("scopes", &self.scopes)
            .field("token_type", &self.token_type)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

impl Drop for TwitchCredential {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ValidatedToken {
    pub client_id: String,
    pub login: String,
    pub user_id: String,
    pub scopes: Vec<String>,
    pub expires_in: u64,
}

#[derive(Clone)]
pub struct TwitchOAuthClient {
    client: Client,
    device_endpoint: Url,
    token_endpoint: Url,
    validate_endpoint: Url,
    revoke_endpoint: Url,
}

impl TwitchOAuthClient {
    pub fn new() -> Result<Self, TwitchError> {
        Self::with_endpoints(DEVICE_ENDPOINT, TOKEN_ENDPOINT, VALIDATE_ENDPOINT, REVOKE_ENDPOINT)
    }

    fn with_endpoints(
        device_endpoint: &str,
        token_endpoint: &str,
        validate_endpoint: &str,
        revoke_endpoint: &str,
    ) -> Result<Self, TwitchError> {
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent("SemanticEngine/0.1")
            .build()
            .map_err(TwitchError::transport)?;
        Ok(Self {
            client,
            device_endpoint: parse_https_or_loopback(device_endpoint)?,
            token_endpoint: parse_https_or_loopback(token_endpoint)?,
            validate_endpoint: parse_https_or_loopback(validate_endpoint)?,
            revoke_endpoint: parse_https_or_loopback(revoke_endpoint)?,
        })
    }

    pub async fn begin_device_authorization(
        &self,
        client_id: &str,
        now_ms: u64,
    ) -> Result<PendingDeviceAuthorization, TwitchError> {
        validate_client_id(client_id)?;
        let scopes = vec![CHAT_SCOPE.to_owned()];
        let response = self
            .client
            .post(self.device_endpoint.clone())
            .form(&[("client_id", client_id), ("scopes", CHAT_SCOPE)])
            .send()
            .await
            .map_err(TwitchError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        let payload: DeviceResponse = parse_json(&body)?;
        validate_device_response(&payload)?;
        let expires_at_ms = now_ms.saturating_add(payload.expires_in.saturating_mul(1_000));
        let prompt = DeviceAuthorizationPrompt {
            user_code: payload.user_code,
            verification_uri: payload.verification_uri,
            expires_at_ms,
            poll_interval_seconds: payload.interval,
        };
        Ok(PendingDeviceAuthorization {
            client_id: client_id.to_owned(),
            device_code: SecretString(payload.device_code),
            scopes,
            prompt,
        })
    }

    pub async fn poll_device_authorization(
        &self,
        pending: &PendingDeviceAuthorization,
        now_ms: u64,
    ) -> Result<DevicePoll, TwitchError> {
        if pending.is_expired_at(now_ms) {
            return Err(TwitchError::Expired);
        }
        let scopes = pending.scopes.join(" ");
        let response = self
            .client
            .post(self.token_endpoint.clone())
            .form(&[
                ("client_id", pending.client_id.as_str()),
                ("scopes", scopes.as_str()),
                ("device_code", pending.device_code.expose()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(TwitchError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if status.is_success() {
            let payload: TokenResponse = parse_json(&body)?;
            return Ok(DevicePoll::Authorized(payload.into_credential(now_ms)?));
        }
        let message = api_message(&body);
        match message.as_deref() {
            Some("authorization_pending") => Ok(DevicePoll::Pending),
            Some("slow_down") => Ok(DevicePoll::SlowDown),
            Some("access_denied") => Err(TwitchError::AccessDenied),
            Some("expired_token" | "invalid device code") => Err(TwitchError::Expired),
            _ => Err(TwitchError::Api {
                status: status.as_u16(),
                message: public_api_message(message),
            }),
        }
    }

    pub async fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
        now_ms: u64,
    ) -> Result<TwitchCredential, TwitchError> {
        validate_client_id(client_id)?;
        validate_token(refresh_token)?;
        let response = self
            .client
            .post(self.token_endpoint.clone())
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", client_id),
            ])
            .send()
            .await
            .map_err(TwitchError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        parse_json::<TokenResponse>(&body)?.into_credential(now_ms)
    }

    pub async fn validate(&self, access_token: &str) -> Result<ValidatedToken, TwitchError> {
        validate_token(access_token)?;
        let response = self
            .client
            .get(self.validate_endpoint.clone())
            .header("Authorization", format!("OAuth {access_token}"))
            .send()
            .await
            .map_err(TwitchError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        let validated: ValidatedToken = parse_json(&body)?;
        if validated.client_id.is_empty()
            || validated.user_id.is_empty()
            || validated.login.is_empty()
            || validated.expires_in == 0
        {
            return Err(TwitchError::InvalidResponse);
        }
        Ok(validated)
    }

    pub async fn revoke(&self, client_id: &str, access_token: &str) -> Result<(), TwitchError> {
        validate_client_id(client_id)?;
        validate_token(access_token)?;
        let response = self
            .client
            .post(self.revoke_endpoint.clone())
            .form(&[("client_id", client_id), ("token", access_token)])
            .send()
            .await
            .map_err(TwitchError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if status.is_success() { Ok(()) } else { Err(api_error(status, &body)) }
    }
}

pub fn store_credential(
    vault: &dyn CredentialVault,
    credential_id: &str,
    credential: &TwitchCredential,
) -> Result<(), TwitchError> {
    let mut bytes = serde_json::to_vec(credential).map_err(|_| TwitchError::InvalidResponse)?;
    let result = vault.store(credential_id, &bytes).map_err(TwitchError::Vault);
    bytes.zeroize();
    result
}

pub fn load_credential(
    vault: &dyn CredentialVault,
    credential_id: &str,
) -> Result<TwitchCredential, TwitchError> {
    let secret = vault.load(credential_id).map_err(TwitchError::Vault)?;
    let credential: TwitchCredential =
        serde_json::from_slice(secret.expose()).map_err(|_| TwitchError::InvalidResponse)?;
    credential.validate()?;
    Ok(credential)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TwitchError {
    InvalidConfig(&'static str),
    InvalidResponse,
    ResponseTooLarge,
    Expired,
    AccessDenied,
    Api { status: u16, message: String },
    Transport(String),
    Vault(VaultError),
    Backpressure,
}

impl TwitchError {
    fn transport(error: impl fmt::Display) -> Self {
        Self::Transport(error.to_string())
    }
}

impl fmt::Display for TwitchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(reason) => {
                write!(formatter, "invalid Twitch configuration: {reason}")
            }
            Self::InvalidResponse => write!(formatter, "Twitch returned an invalid response"),
            Self::ResponseTooLarge => write!(formatter, "Twitch response exceeds the size limit"),
            Self::Expired => write!(formatter, "Twitch authorization expired"),
            Self::AccessDenied => write!(formatter, "Twitch authorization was denied"),
            Self::Api { status, message } => {
                write!(formatter, "Twitch API error {status}: {message}")
            }
            Self::Transport(message) => write!(formatter, "Twitch transport error: {message}"),
            Self::Vault(error) => write!(formatter, "Twitch credential error: {error}"),
            Self::Backpressure => write!(formatter, "Twitch source output is backpressured"),
        }
    }
}

impl std::error::Error for TwitchError {}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    expires_in: u64,
    interval: u64,
    user_code: String,
    verification_uri: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    scope: Vec<String>,
    token_type: String,
    expires_in: u64,
}

impl TokenResponse {
    fn into_credential(self, now_ms: u64) -> Result<TwitchCredential, TwitchError> {
        validate_token(&self.access_token)?;
        validate_token(&self.refresh_token)?;
        if !self.token_type.eq_ignore_ascii_case("bearer")
            || self.expires_in == 0
            || !self.scope.iter().any(|scope| scope == CHAT_SCOPE)
            || self.scope.len() > 32
        {
            return Err(TwitchError::InvalidResponse);
        }
        Ok(TwitchCredential {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            scopes: self.scope,
            token_type: self.token_type,
            expires_at_ms: now_ms.saturating_add(self.expires_in.saturating_mul(1_000)),
        })
    }
}

impl TwitchCredential {
    fn validate(&self) -> Result<(), TwitchError> {
        validate_token(&self.access_token)?;
        validate_token(&self.refresh_token)?;
        if !self.token_type.eq_ignore_ascii_case("bearer")
            || self.expires_at_ms == 0
            || !self.scopes.iter().any(|scope| scope == CHAT_SCOPE)
            || self.scopes.len() > 32
        {
            return Err(TwitchError::InvalidResponse);
        }
        Ok(())
    }
}

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

async fn read_limited(response: reqwest::Response) -> Result<Vec<u8>, TwitchError> {
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(TwitchError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(TwitchError::transport)?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(TwitchError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_device_response(payload: &DeviceResponse) -> Result<(), TwitchError> {
    if payload.device_code.is_empty()
        || payload.device_code.chars().count() > MAX_DEVICE_CODE_CHARS
        || payload.user_code.is_empty()
        || payload.user_code.chars().count() > MAX_USER_CODE_CHARS
        || payload.user_code.chars().any(char::is_control)
        || !(MIN_POLL_INTERVAL_SECONDS..=MAX_POLL_INTERVAL_SECONDS).contains(&payload.interval)
        || payload.expires_in == 0
        || payload.expires_in > MAX_DEVICE_LIFETIME_SECONDS
    {
        return Err(TwitchError::InvalidResponse);
    }
    let uri = Url::parse(&payload.verification_uri).map_err(|_| TwitchError::InvalidResponse)?;
    if uri.scheme() != "https" || uri.host_str().is_none() {
        return Err(TwitchError::InvalidResponse);
    }
    Ok(())
}

fn validate_client_id(value: &str) -> Result<(), TwitchError> {
    if value.is_empty()
        || value.chars().count() > MAX_CLIENT_ID_CHARS
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(TwitchError::InvalidConfig("client ID is invalid"));
    }
    Ok(())
}

fn validate_token(value: &str) -> Result<(), TwitchError> {
    if value.is_empty()
        || value.chars().count() > MAX_TOKEN_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(TwitchError::InvalidResponse);
    }
    Ok(())
}

fn parse_https_or_loopback(value: &str) -> Result<Url, TwitchError> {
    let url = Url::parse(value).map_err(|_| TwitchError::InvalidConfig("endpoint is invalid"))?;
    let is_loopback = url.host_str().is_some_and(|host| host == "127.0.0.1" || host == "localhost");
    if (url.scheme() != "https" && !(cfg!(test) && url.scheme() == "http" && is_loopback))
        || url.host_str().is_none()
    {
        return Err(TwitchError::InvalidConfig("endpoint must use HTTPS"));
    }
    Ok(url)
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, TwitchError> {
    serde_json::from_slice(body).map_err(|_| TwitchError::InvalidResponse)
}

fn api_error(status: StatusCode, body: &[u8]) -> TwitchError {
    let message = api_message(body);
    TwitchError::Api { status: status.as_u16(), message: public_api_message(message) }
}

fn api_message(body: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrorBody {
        message: String,
    }
    serde_json::from_slice::<ErrorBody>(body).ok().map(|payload| payload.message)
}

fn public_api_message(message: Option<String>) -> String {
    message
        .filter(|value| value.chars().count() <= 256 && !value.chars().any(char::is_control))
        .unwrap_or_else(|| "request refused".to_owned())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use axum::{
        Json, Router,
        extract::{Form, State},
        http::StatusCode,
        routing::{get, post},
    };
    use semantic_engine_credential_vault::{CredentialVault, SecretValue};

    use super::*;

    #[derive(Default)]
    struct MemoryVault(Mutex<HashMap<String, Vec<u8>>>);

    impl CredentialVault for MemoryVault {
        fn store(&self, credential_id: &str, secret: &[u8]) -> Result<(), VaultError> {
            self.0.lock().unwrap().insert(credential_id.to_owned(), secret.to_vec());
            Ok(())
        }

        fn load(&self, credential_id: &str) -> Result<SecretValue, VaultError> {
            let bytes =
                self.0.lock().unwrap().get(credential_id).cloned().ok_or(VaultError::Missing)?;
            SecretValue::new(bytes)
        }

        fn delete(&self, credential_id: &str) -> Result<(), VaultError> {
            self.0.lock().unwrap().remove(credential_id);
            Ok(())
        }
    }

    #[derive(Default)]
    struct ApiState(Mutex<u64>);

    #[tokio::test]
    async fn device_flow_handles_pending_then_stores_only_in_the_vault() {
        let state = std::sync::Arc::new(ApiState::default());
        let app = Router::new()
            .route(
                "/device",
                post(|| async {
                    Json(serde_json::json!({
                        "device_code": "private-device-code",
                        "expires_in": 1800,
                        "interval": 1,
                        "user_code": "ABCDEFGH",
                        "verification_uri": "https://www.twitch.tv/activate"
                    }))
                }),
            )
            .route(
                "/token",
                post(|State(state): State<std::sync::Arc<ApiState>>| async move {
                    let mut count = state.0.lock().unwrap();
                    *count += 1;
                    if *count == 1 {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({"message": "authorization_pending"})),
                        );
                    }
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "access_token": "access-token",
                            "refresh_token": "refresh-token",
                            "scope": [CHAT_SCOPE],
                            "token_type": "bearer",
                            "expires_in": 14400
                        })),
                    )
                }),
            )
            .route(
                "/validate",
                get(|| async {
                    Json(serde_json::json!({
                        "client_id": "client123",
                        "login": "streamer",
                        "user_id": "42",
                        "scopes": [CHAT_SCOPE],
                        "expires_in": 14000
                    }))
                }),
            )
            .route(
                "/revoke",
                post(|Form(form): Form<HashMap<String, String>>| async move {
                    assert_eq!(form.get("client_id").map(String::as_str), Some("client123"));
                    assert_eq!(form.get("token").map(String::as_str), Some("access-token"));
                    StatusCode::OK
                }),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = TwitchOAuthClient::with_endpoints(
            &format!("http://{address}/device"),
            &format!("http://{address}/token"),
            &format!("http://{address}/validate"),
            &format!("http://{address}/revoke"),
        )
        .unwrap();

        let pending = client.begin_device_authorization("client123", 1_000).await.unwrap();
        assert_eq!(pending.prompt().user_code, "ABCDEFGH");
        assert!(format!("{pending:?}").contains("[REDACTED]"));
        assert_eq!(
            client.poll_device_authorization(&pending, 2_000).await.unwrap(),
            DevicePoll::Pending
        );
        let DevicePoll::Authorized(credential) =
            client.poll_device_authorization(&pending, 3_000).await.unwrap()
        else {
            panic!("second poll should authorize");
        };
        assert_eq!(client.validate(credential.access_token()).await.unwrap().user_id, "42");
        client.revoke("client123", credential.access_token()).await.unwrap();

        let vault = MemoryVault::default();
        store_credential(&vault, "twitch-main", &credential).unwrap();
        let restored = load_credential(&vault, "twitch-main").unwrap();
        assert_eq!(restored.access_token(), "access-token");
        assert!(!format!("{restored:?}").contains("access-token"));
    }

    #[test]
    fn verification_urls_and_tokens_are_bounded() {
        let invalid = DeviceResponse {
            device_code: "device".to_owned(),
            expires_in: 1800,
            interval: 1,
            user_code: "code".to_owned(),
            verification_uri: "http://attacker.invalid/activate".to_owned(),
        };
        assert_eq!(validate_device_response(&invalid), Err(TwitchError::InvalidResponse));
        assert!(validate_token(&"x".repeat(MAX_TOKEN_CHARS + 1)).is_err());
    }
}
