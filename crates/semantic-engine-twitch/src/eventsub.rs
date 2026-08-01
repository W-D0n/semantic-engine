use std::{collections::VecDeque, fmt, time::Duration};

use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use semantic_engine_core::{MAX_IDENTIFIER_CHARS, MAX_SUBMISSION_CHARS};
use semantic_engine_source::{SourceAdapterEvent, SourceMessage, SourceRuntimeState};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

use crate::{TwitchCredential, TwitchError, read_limited};

const WEBSOCKET_ENDPOINT: &str = "wss://eventsub.wss.twitch.tv/ws";
const SUBSCRIPTIONS_ENDPOINT: &str = "https://api.twitch.tv/helix/eventsub/subscriptions";
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_NOTIFICATION_IDS: usize = 4_096;
const MAX_KEEPALIVE_SECONDS: u64 = 600;
const MIN_KEEPALIVE_SECONDS: u64 = 10;
const WELCOME_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventSubConfig {
    pub source_id: String,
    pub client_id: String,
    pub broadcaster_user_id: String,
    pub user_id: String,
    pub next_source_sequence: u64,
}

#[derive(Clone)]
pub struct TwitchEventSubClient {
    http: Client,
    websocket_endpoint: Url,
    subscriptions_endpoint: Url,
}

impl TwitchEventSubClient {
    pub fn new() -> Result<Self, TwitchError> {
        Self::with_endpoints(WEBSOCKET_ENDPOINT, SUBSCRIPTIONS_ENDPOINT)
    }

    fn with_endpoints(
        websocket_endpoint: &str,
        subscriptions_endpoint: &str,
    ) -> Result<Self, TwitchError> {
        let http = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent("SemanticEngine/0.1")
            .build()
            .map_err(TwitchError::transport)?;
        Ok(Self {
            http,
            websocket_endpoint: parse_websocket_endpoint(websocket_endpoint)?,
            subscriptions_endpoint: super::parse_https_or_loopback(subscriptions_endpoint)?,
        })
    }

    pub async fn run(
        &self,
        config: EventSubConfig,
        credential: &TwitchCredential,
        output: mpsc::Sender<SourceAdapterEvent>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), TwitchError> {
        validate_config(&config)?;
        let mut decoder =
            EventSubDecoder::new(config.source_id.clone(), config.next_source_sequence);
        let mut endpoint = self.websocket_endpoint.clone();
        let mut subscribe_after_welcome = true;
        let mut backoff = Duration::from_secs(1);

        loop {
            if *shutdown.borrow() {
                emit_state(&output, &config.source_id, SourceRuntimeState::Paused, None)?;
                return Ok(());
            }
            emit_state(&output, &config.source_id, SourceRuntimeState::Connecting, None)?;
            match self
                .run_connection(
                    &endpoint,
                    subscribe_after_welcome,
                    &config,
                    credential,
                    &mut decoder,
                    &output,
                    &mut shutdown,
                )
                .await
            {
                Ok(ConnectionExit::Shutdown) => {
                    emit_state(&output, &config.source_id, SourceRuntimeState::Paused, None)?;
                    return Ok(());
                }
                Ok(ConnectionExit::Reconnect(url)) => {
                    endpoint = url;
                    subscribe_after_welcome = false;
                    backoff = Duration::from_secs(1);
                }
                Ok(ConnectionExit::Lost) | Err(TwitchError::Transport(_)) => {
                    emit_state(
                        &output,
                        &config.source_id,
                        SourceRuntimeState::Backoff,
                        Some(format!("retry in {}s", backoff.as_secs())),
                    )?;
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                emit_state(&output, &config.source_id, SourceRuntimeState::Paused, None)?;
                                return Ok(());
                            }
                        }
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    endpoint = self.websocket_endpoint.clone();
                    subscribe_after_welcome = true;
                }
                Err(error) => {
                    emit_state(
                        &output,
                        &config.source_id,
                        SourceRuntimeState::Faulted,
                        Some(error.public_code().to_owned()),
                    )?;
                    return Err(error);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_connection(
        &self,
        endpoint: &Url,
        subscribe_after_welcome: bool,
        config: &EventSubConfig,
        credential: &TwitchCredential,
        decoder: &mut EventSubDecoder,
        output: &mpsc::Sender<SourceAdapterEvent>,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<ConnectionExit, TwitchError> {
        let websocket_config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME_BYTES))
            .max_frame_size(Some(MAX_FRAME_BYTES));
        let (mut socket, response) =
            connect_async_with_config(endpoint.as_str(), Some(websocket_config), true)
                .await
                .map_err(TwitchError::transport)?;
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(TwitchError::Transport(
                "EventSub WebSocket upgrade was refused".to_owned(),
            ));
        }

        let welcome = tokio::time::timeout(WELCOME_TIMEOUT, socket.next())
            .await
            .map_err(|_| TwitchError::Transport("EventSub welcome timed out".to_owned()))?
            .ok_or_else(|| TwitchError::Transport("EventSub closed before welcome".to_owned()))?
            .map_err(TwitchError::transport)?;
        let Message::Text(welcome) = welcome else {
            return Err(TwitchError::InvalidResponse);
        };
        let DecodedFrame::Welcome { session_id, keepalive_seconds } =
            decoder.decode(welcome.as_str(), now_ms())?
        else {
            return Err(TwitchError::InvalidResponse);
        };
        if subscribe_after_welcome {
            self.subscribe(config, credential, &session_id).await?;
        }
        emit_state(output, &config.source_id, SourceRuntimeState::Connected, None)?;
        let receive_timeout = Duration::from_secs(keepalive_seconds.saturating_add(5));

        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        let _ = socket.close(None).await;
                        return Ok(ConnectionExit::Shutdown);
                    }
                }
                incoming = tokio::time::timeout(receive_timeout, socket.next()) => {
                    let frame = incoming
                        .map_err(|_| TwitchError::Transport("EventSub keepalive timed out".to_owned()))?
                        .ok_or_else(|| TwitchError::Transport("EventSub connection closed".to_owned()))?
                        .map_err(TwitchError::transport)?;
                    match frame {
                        Message::Text(text) => match decoder.decode(text.as_str(), now_ms())? {
                            DecodedFrame::Notification(message) => output
                                .try_send(SourceAdapterEvent::Message(message))
                                .map_err(|_| TwitchError::Backpressure)?,
                            DecodedFrame::Reconnect(url) => return Ok(ConnectionExit::Reconnect(url)),
                            DecodedFrame::Revoked(code) => {
                                return Err(TwitchError::Api { status: 409, message: code });
                            }
                            DecodedFrame::Keepalive | DecodedFrame::Duplicate => {}
                            DecodedFrame::Welcome { .. } => return Err(TwitchError::InvalidResponse),
                        },
                        Message::Ping(payload) => socket
                            .send(Message::Pong(payload))
                            .await
                            .map_err(TwitchError::transport)?,
                        Message::Close(_) => return Ok(ConnectionExit::Lost),
                        Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
                    }
                }
            }
        }
    }

    async fn subscribe(
        &self,
        config: &EventSubConfig,
        credential: &TwitchCredential,
        session_id: &str,
    ) -> Result<(), TwitchError> {
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "channel.chat.message",
            "version": "1",
            "condition": {
                "broadcaster_user_id": config.broadcaster_user_id,
                "user_id": config.user_id
            },
            "transport": {
                "method": "websocket",
                "session_id": session_id
            }
        }))
        .map_err(|_| TwitchError::InvalidResponse)?;
        let response = self
            .http
            .post(self.subscriptions_endpoint.clone())
            .header("Client-Id", &config.client_id)
            .bearer_auth(credential.access_token())
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(TwitchError::transport)?;
        let status = response.status();
        let body = read_limited(response).await?;
        if status != StatusCode::ACCEPTED {
            return Err(super::api_error(status, &body));
        }
        Ok(())
    }
}

enum ConnectionExit {
    Shutdown,
    Reconnect(Url),
    Lost,
}

#[derive(Deserialize)]
struct Envelope {
    metadata: Metadata,
    payload: Payload,
}

#[derive(Deserialize)]
struct Metadata {
    message_id: String,
    message_type: String,
}

#[derive(Default, Deserialize)]
struct Payload {
    #[serde(default)]
    session: Option<SessionPayload>,
    #[serde(default)]
    subscription: Option<SubscriptionPayload>,
    #[serde(default)]
    event: Option<ChatEvent>,
}

#[derive(Deserialize)]
struct SessionPayload {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    keepalive_timeout_seconds: Option<u64>,
    #[serde(default)]
    reconnect_url: Option<String>,
}

#[derive(Deserialize)]
struct SubscriptionPayload {
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
struct ChatEvent {
    message_id: String,
    chatter_user_id: String,
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    text: String,
}

enum DecodedFrame {
    Welcome { session_id: String, keepalive_seconds: u64 },
    Notification(SourceMessage),
    Keepalive,
    Reconnect(Url),
    Revoked(String),
    Duplicate,
}

struct EventSubDecoder {
    source_id: String,
    next_sequence: u64,
    recent_notification_ids: VecDeque<String>,
}

impl EventSubDecoder {
    fn new(source_id: String, next_sequence: u64) -> Self {
        Self { source_id, next_sequence, recent_notification_ids: VecDeque::new() }
    }

    fn decode(&mut self, text: &str, occurred_at_ms: u64) -> Result<DecodedFrame, TwitchError> {
        let envelope: Envelope =
            serde_json::from_str(text).map_err(|_| TwitchError::InvalidResponse)?;
        validate_identifier(&envelope.metadata.message_id)?;
        match envelope.metadata.message_type.as_str() {
            "session_welcome" => {
                let session = envelope.payload.session.ok_or(TwitchError::InvalidResponse)?;
                let session_id = session.id.ok_or(TwitchError::InvalidResponse)?;
                validate_identifier(&session_id)?;
                let keepalive_seconds = session
                    .keepalive_timeout_seconds
                    .filter(|value| (MIN_KEEPALIVE_SECONDS..=MAX_KEEPALIVE_SECONDS).contains(value))
                    .ok_or(TwitchError::InvalidResponse)?;
                Ok(DecodedFrame::Welcome { session_id, keepalive_seconds })
            }
            "session_keepalive" => Ok(DecodedFrame::Keepalive),
            "session_reconnect" => {
                let reconnect = envelope
                    .payload
                    .session
                    .and_then(|session| session.reconnect_url)
                    .ok_or(TwitchError::InvalidResponse)?;
                Ok(DecodedFrame::Reconnect(parse_websocket_endpoint(&reconnect)?))
            }
            "revocation" => Ok(DecodedFrame::Revoked(
                envelope
                    .payload
                    .subscription
                    .map_or_else(|| "revoked".to_owned(), |subscription| subscription.status),
            )),
            "notification" => {
                if self.recent_notification_ids.iter().any(|id| id == &envelope.metadata.message_id)
                {
                    return Ok(DecodedFrame::Duplicate);
                }
                let event = envelope.payload.event.ok_or(TwitchError::InvalidResponse)?;
                validate_identifier(&event.message_id)?;
                validate_identifier(&event.chatter_user_id)?;
                let message_id = format!("{}:{}", self.source_id, event.message_id);
                validate_identifier(&message_id)?;
                if event.message.text.chars().count() > MAX_SUBMISSION_CHARS
                    || event.message.text.chars().any(|character| character == '\0')
                {
                    return Err(TwitchError::InvalidResponse);
                }
                let sequence = self.next_sequence;
                self.next_sequence =
                    self.next_sequence.checked_add(1).ok_or(TwitchError::InvalidResponse)?;
                self.recent_notification_ids.push_back(envelope.metadata.message_id);
                if self.recent_notification_ids.len() > MAX_NOTIFICATION_IDS {
                    self.recent_notification_ids.pop_front();
                }
                Ok(DecodedFrame::Notification(SourceMessage {
                    source_id: self.source_id.clone(),
                    message_id,
                    participant_id: event.chatter_user_id,
                    source_sequence: sequence,
                    text: event.message.text,
                    occurred_at_ms,
                }))
            }
            _ => Err(TwitchError::InvalidResponse),
        }
    }
}

fn validate_config(config: &EventSubConfig) -> Result<(), TwitchError> {
    validate_identifier(&config.source_id)?;
    super::validate_client_id(&config.client_id)?;
    validate_identifier(&config.broadcaster_user_id)?;
    validate_identifier(&config.user_id)
}

fn validate_identifier(value: &str) -> Result<(), TwitchError> {
    if value.is_empty()
        || value.chars().count() > MAX_IDENTIFIER_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(TwitchError::InvalidResponse);
    }
    Ok(())
}

fn parse_websocket_endpoint(value: &str) -> Result<Url, TwitchError> {
    let url = Url::parse(value)
        .map_err(|_| TwitchError::InvalidConfig("WebSocket endpoint is invalid"))?;
    let loopback = url.host_str().is_some_and(|host| host == "127.0.0.1" || host == "localhost");
    let production = url.scheme() == "wss" && url.host_str() == Some("eventsub.wss.twitch.tv");
    let local_test = cfg!(test) && url.scheme() == "ws" && loopback;
    if !production && !local_test {
        return Err(TwitchError::InvalidConfig("WebSocket endpoint is not trusted"));
    }
    Ok(url)
}

fn emit_state(
    output: &mpsc::Sender<SourceAdapterEvent>,
    source_id: &str,
    state: SourceRuntimeState,
    detail: Option<String>,
) -> Result<(), TwitchError> {
    output
        .try_send(SourceAdapterEvent::StateChanged {
            source_id: source_id.to_owned(),
            state,
            detail,
        })
        .map_err(|_| TwitchError::Backpressure)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

impl TwitchError {
    fn public_code(&self) -> &'static str {
        match self {
            Self::InvalidConfig(_) => "invalid_config",
            Self::InvalidResponse => "invalid_response",
            Self::ResponseTooLarge => "response_too_large",
            Self::Expired => "authorization_expired",
            Self::AccessDenied => "access_denied",
            Self::Api { .. } => "api_error",
            Self::Transport(_) => "transport_error",
            Self::Vault(_) => "credential_error",
            Self::Backpressure => "backpressure",
        }
    }
}

impl fmt::Debug for EventSubDecoder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventSubDecoder")
            .field("source_id", &self.source_id)
            .field("next_sequence", &self.next_sequence)
            .field("dedup_entries", &self.recent_notification_ids.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, http::StatusCode, routing::post};

    use super::*;

    #[test]
    fn notifications_are_bounded_deduplicated_and_translated() {
        let mut decoder = EventSubDecoder::new("twitch-main".to_owned(), 7);
        let notification = serde_json::json!({
            "metadata": {"message_id": "notification-1", "message_type": "notification"},
            "payload": {
                "event": {
                    "message_id": "chat-message-1",
                    "chatter_user_id": "viewer-42",
                    "message": {"text": "eldern ring"}
                }
            }
        })
        .to_string();
        let DecodedFrame::Notification(message) = decoder.decode(&notification, 123).unwrap()
        else {
            panic!("notification should decode");
        };
        assert_eq!(message.source_sequence, 7);
        assert_eq!(message.text, "eldern ring");
        assert!(matches!(decoder.decode(&notification, 124), Ok(DecodedFrame::Duplicate)));
        assert_eq!(decoder.next_sequence, 8);
        assert!(!format!("{decoder:?}").contains("eldern ring"));
    }

    #[test]
    fn reconnect_urls_are_restricted_to_twitch() {
        assert!(parse_websocket_endpoint("wss://eventsub.wss.twitch.tv/ws").is_ok());
        assert!(parse_websocket_endpoint("wss://attacker.invalid/ws").is_err());
    }

    #[tokio::test]
    async fn real_socket_flow_subscribes_and_emits_a_bounded_source_message() {
        let websocket_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let websocket_address = websocket_listener.local_addr().unwrap();
        let websocket_server = tokio::spawn(async move {
            let (stream, _) = websocket_listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "metadata": {"message_id": "welcome-1", "message_type": "session_welcome"},
                        "payload": {"session": {
                            "id": "session-1",
                            "keepalive_timeout_seconds": 10,
                            "reconnect_url": null
                        }}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "metadata": {"message_id": "notification-1", "message_type": "notification"},
                        "payload": {"event": {
                            "message_id": "chat-message-1",
                            "chatter_user_id": "viewer-42",
                            "message": {"text": "elden rings"}
                        }}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            while socket.next().await.is_some() {}
        });

        let http_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_address = http_listener.local_addr().unwrap();
        let http_server = tokio::spawn(async move {
            axum::serve(
                http_listener,
                Router::new()
                    .route("/subscriptions", post(|| async { (StatusCode::ACCEPTED, "{}") })),
            )
            .await
            .unwrap();
        });

        let client = TwitchEventSubClient::with_endpoints(
            &format!("ws://{websocket_address}/ws"),
            &format!("http://{http_address}/subscriptions"),
        )
        .unwrap();
        let credential = TwitchCredential {
            access_token: "access-token".to_owned(),
            refresh_token: "refresh-token".to_owned(),
            scopes: vec![crate::CHAT_SCOPE.to_owned()],
            token_type: "bearer".to_owned(),
            expires_at_ms: u64::MAX,
        };
        let (output, mut events) = mpsc::channel(8);
        let (shutdown, receiver) = watch::channel(false);
        let task = tokio::spawn(async move {
            client
                .run(
                    EventSubConfig {
                        source_id: "twitch-main".to_owned(),
                        client_id: "client123".to_owned(),
                        broadcaster_user_id: "42".to_owned(),
                        user_id: "42".to_owned(),
                        next_source_sequence: 9,
                    },
                    &credential,
                    output,
                    receiver,
                )
                .await
        });

        let mut delivered = None;
        while let Some(event) =
            tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap()
        {
            if let SourceAdapterEvent::Message(message) = event {
                delivered = Some(message);
                break;
            }
        }
        let delivered = delivered.expect("chat message should be delivered");
        assert_eq!(delivered.source_sequence, 9);
        assert_eq!(delivered.participant_id, "viewer-42");
        assert_eq!(delivered.text, "elden rings");
        shutdown.send(true).unwrap();
        task.await.unwrap().unwrap();
        websocket_server.await.unwrap();
        http_server.abort();
    }
}
