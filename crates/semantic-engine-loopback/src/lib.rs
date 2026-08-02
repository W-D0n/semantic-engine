use std::{
    collections::VecDeque,
    fmt,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, Path, Query, State, WebSocketUpgrade,
        rejection::{BytesRejection, QueryRejection},
        ws::Message,
    },
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post},
};
pub use semantic_engine_protocol::PROTOCOL_VERSION;
use semantic_engine_protocol::{
    Command, RequestEnvelope, ResponseStatus, handle, handle_json_line,
};
use semantic_engine_service::SemanticEngineService;
use semantic_engine_source_runtime::SourceRuntime;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore, oneshot, watch},
    task::JoinHandle,
    time,
};
use tower_http::cors::CorsLayer;

pub const DEFAULT_PORT: u16 = 17_831;
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const HTTP_PROTOCOL_HEADER: &str = "x-semantic-engine-protocol";
pub const WEBSOCKET_PROTOCOL: &str = "semantic-engine.v2";
const WEBSOCKET_TOKEN_PREFIX: &str = "semantic-engine.token.";
pub type SharedService = Arc<Mutex<SemanticEngineService>>;

#[derive(Clone, Debug)]
pub struct LoopbackConfig {
    pub bind_addr: SocketAddr,
    pub allowed_origins: Vec<String>,
    pub max_requests_per_second: usize,
    pub max_in_flight: usize,
    pub max_websocket_connections: usize,
    pub event_poll_interval: Duration,
}

impl Default for LoopbackConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT),
            allowed_origins: Vec::new(),
            max_requests_per_second: 100,
            max_in_flight: 32,
            max_websocket_connections: 8,
            event_poll_interval: Duration::from_millis(100),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopbackError {
    InvalidConfig(String),
    Entropy(String),
    Io(String),
    Task(String),
}

impl fmt::Display for LoopbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(formatter, "invalid loopback config: {message}"),
            Self::Entropy(message) => write!(formatter, "token generation failed: {message}"),
            Self::Io(message) => write!(formatter, "loopback I/O failed: {message}"),
            Self::Task(message) => write!(formatter, "loopback task failed: {message}"),
        }
    }
}

impl std::error::Error for LoopbackError {}

pub struct LoopbackServer {
    addr: SocketAddr,
    token: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), std::io::Error>>>,
}

impl LoopbackServer {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn is_running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }

    pub async fn shutdown(mut self) -> Result<(), LoopbackError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.join().await
    }

    async fn join(&mut self) -> Result<(), LoopbackError> {
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        task.await
            .map_err(|error| LoopbackError::Task(error.to_string()))?
            .map_err(|error| LoopbackError::Io(error.to_string()))
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

#[derive(Clone)]
struct AppState {
    service: SharedService,
    sources: Option<Arc<SourceRuntime>>,
    token_hash: [u8; 32],
    allowed_origins: Arc<Vec<String>>,
    request_quota: Arc<StdMutex<VecDeque<Instant>>>,
    max_requests_per_second: usize,
    in_flight: Arc<Semaphore>,
    websocket_connections: Arc<Semaphore>,
    event_poll_interval: Duration,
    shutdown: watch::Receiver<bool>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    transport: &'static str,
    protocol_versions: [u32; 1],
}

#[derive(Serialize)]
struct TransportErrorEnvelope {
    error: TransportErrorBody,
}

#[derive(Serialize)]
struct TransportErrorBody {
    code: &'static str,
    message: &'static str,
    retryable: bool,
}

#[derive(Clone, Copy)]
struct TransportFailure {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retryable: bool,
}

impl TransportFailure {
    fn response(self) -> Response {
        transport_error(self.status, self.code, self.message, self.retryable)
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    session_id: String,
    #[serde(default)]
    after_sequence: u64,
    #[serde(default = "default_event_limit")]
    limit: usize,
}

#[derive(Deserialize)]
struct CreateTwitchSourceRequest {
    display_name: String,
    client_id: String,
}

#[derive(Deserialize)]
struct CreateYouTubeSourceRequest {
    display_name: String,
    client_id: String,
    #[serde(default)]
    video_id: String,
    policy_acknowledged: bool,
}

#[derive(Deserialize)]
struct StartSourceRequest {
    expected_revision: u64,
    session_id: String,
}

#[derive(Deserialize)]
struct SelectYouTubeBroadcastRequest {
    expected_revision: u64,
    video_id: String,
}

#[derive(Deserialize)]
struct DeleteSourceQuery {
    expected_revision: u64,
}

pub async fn start(
    service: SemanticEngineService,
    config: LoopbackConfig,
) -> Result<LoopbackServer, LoopbackError> {
    start_shared(Arc::new(Mutex::new(service)), config).await
}

pub async fn start_shared(
    service: SharedService,
    config: LoopbackConfig,
) -> Result<LoopbackServer, LoopbackError> {
    start_shared_inner(service, None, config).await
}

pub async fn start_shared_with_sources(
    service: SharedService,
    sources: Arc<SourceRuntime>,
    config: LoopbackConfig,
) -> Result<LoopbackServer, LoopbackError> {
    start_shared_inner(service, Some(sources), config).await
}

async fn start_shared_inner(
    service: SharedService,
    sources: Option<Arc<SourceRuntime>>,
    config: LoopbackConfig,
) -> Result<LoopbackServer, LoopbackError> {
    validate_config(&config)?;
    let origins = config
        .allowed_origins
        .iter()
        .map(|origin| {
            origin.parse::<HeaderValue>().map_err(|_| {
                LoopbackError::InvalidConfig(format!(
                    "origin is not a valid header value: {origin}"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let token = generate_token()?;
    let (shutdown_signal, shutdown) = watch::channel(false);
    let state = AppState {
        service,
        sources,
        token_hash: token_hash(&token),
        allowed_origins: Arc::new(config.allowed_origins),
        request_quota: Arc::new(StdMutex::new(VecDeque::new())),
        max_requests_per_second: config.max_requests_per_second,
        in_flight: Arc::new(Semaphore::new(config.max_in_flight)),
        websocket_connections: Arc::new(Semaphore::new(config.max_websocket_connections)),
        event_poll_interval: config.event_poll_interval,
        shutdown,
    };
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static(HTTP_PROTOCOL_HEADER),
        ]);
    let router = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/commands", post(command))
        .route("/v1/events/ws", any(events_websocket))
        .route("/v1/sources", get(list_sources))
        .route("/v1/sources/twitch", post(create_twitch_source))
        .route("/v1/sources/youtube", post(create_youtube_source))
        .route("/v1/sources/{source_id}/authorization", post(begin_twitch_authorization))
        .route("/v1/sources/{source_id}/authorization/poll", post(poll_twitch_authorization))
        .route("/v1/sources/{source_id}/test", post(test_twitch_source))
        .route("/v1/sources/{source_id}/youtube/broadcasts", get(discover_youtube_broadcasts))
        .route("/v1/sources/{source_id}/youtube/broadcast", post(select_youtube_broadcast))
        .route("/v1/sources/{source_id}/start", post(start_twitch_source))
        .route("/v1/sources/{source_id}/pause", post(pause_source))
        .route("/v1/sources/{source_id}", delete(delete_source))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(cors)
        .with_state(state);
    let listener = TcpListener::bind(config.bind_addr)
        .await
        .map_err(|error| LoopbackError::Io(error.to_string()))?;
    let addr = listener.local_addr().map_err(|error| LoopbackError::Io(error.to_string()))?;
    let (shutdown, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
                let _ = shutdown_signal.send(true);
            })
            .await
    });
    Ok(LoopbackServer { addr, token, shutdown: Some(shutdown), task: Some(task) })
}

async fn health() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(HealthResponse {
            status: "ok",
            transport: "loopback",
            protocol_versions: [PROTOCOL_VERSION],
        }),
    )
}

async fn command(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if let Err(error) = authorize_http(&state, &headers) {
        return error.response();
    }
    if let Err(error) = negotiate_http_version(&headers) {
        return error.response();
    }
    if !is_json_content_type(&headers) {
        return transport_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "content-type must be application/json",
            false,
        );
    }
    if let Err(error) = take_quota(&state) {
        return error.response();
    }
    let Ok(_permit) = state.in_flight.clone().try_acquire_owned() else {
        return transport_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "backpressure",
            "too many requests are already in flight",
            true,
        );
    };
    let body = match body {
        Ok(body) => body,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return transport_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "the request body exceeds the 1 MiB transport limit",
                false,
            );
        }
        Err(_) => {
            return transport_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_body",
                "the request body could not be read",
                false,
            );
        }
    };
    let service = state.service.clone();
    let response = match tokio::task::spawn_blocking(move || {
        handle_json_line(&mut service.blocking_lock(), &body)
    })
    .await
    {
        Ok(response) => response,
        Err(_) => {
            return transport_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transport_task_failed",
                "the command worker did not complete",
                true,
            );
        }
    };
    let mut response = Json(response).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn list_sources(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let Some(sources) = state.sources.as_ref() else {
        return source_runtime_unavailable();
    };
    match semantic_engine_source_runtime::list_sources(sources).await {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(_) => transport_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "source_list_failed",
            "input sources could not be listed",
            true,
        ),
    }
}

async fn create_twitch_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let request = match parse_json_body(&headers, body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let Some(sources) = state.sources.as_ref() else {
        return source_runtime_unavailable();
    };
    let CreateTwitchSourceRequest { display_name, client_id } = request;
    match semantic_engine_source_runtime::create_twitch_source(display_name, client_id, sources)
        .await
    {
        Ok(result) => json_response(StatusCode::CREATED, result),
        Err(_) => transport_error(
            StatusCode::BAD_REQUEST,
            "invalid_source",
            "the Twitch source configuration is invalid",
            false,
        ),
    }
}

async fn create_youtube_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let request = match parse_json_body(&headers, body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let Some(sources) = state.sources.as_ref() else {
        return source_runtime_unavailable();
    };
    let CreateYouTubeSourceRequest { display_name, client_id, video_id, policy_acknowledged } =
        request;
    match semantic_engine_source_runtime::create_youtube_source(
        display_name,
        client_id,
        video_id,
        policy_acknowledged,
        sources,
    )
    .await
    {
        Ok(result) => json_response(StatusCode::CREATED, result),
        Err(_) => transport_error(
            StatusCode::BAD_REQUEST,
            "invalid_source",
            "the YouTube source configuration is invalid or policy acknowledgement is missing",
            false,
        ),
    }
}

async fn begin_twitch_authorization(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let Some(sources) = source_runtime_for(&state, &source_id) else {
        return source_request_unavailable(&source_id);
    };
    match semantic_engine_source_runtime::begin_source_authorization(source_id, sources).await {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(_) => transport_error(
            StatusCode::BAD_GATEWAY,
            "source_authorization_failed",
            "source authorization could not be started",
            true,
        ),
    }
}

async fn poll_twitch_authorization(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let Some(sources) = source_runtime_for(&state, &source_id) else {
        return source_request_unavailable(&source_id);
    };
    match semantic_engine_source_runtime::poll_source_authorization(source_id, sources).await {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(_) => transport_error(
            StatusCode::BAD_GATEWAY,
            "source_authorization_failed",
            "source authorization could not be completed",
            true,
        ),
    }
}

async fn test_twitch_source(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let Some(sources) = source_runtime_for(&state, &source_id) else {
        return source_request_unavailable(&source_id);
    };
    match semantic_engine_source_runtime::test_source(source_id, sources).await {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(_) => transport_error(
            StatusCode::BAD_GATEWAY,
            "source_test_failed",
            "the source could not be validated",
            true,
        ),
    }
}

async fn discover_youtube_broadcasts(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let Some(sources) = source_runtime_for(&state, &source_id) else {
        return source_request_unavailable(&source_id);
    };
    match semantic_engine_source_runtime::discover_youtube_broadcasts(source_id, sources).await {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(_) => transport_error(
            StatusCode::BAD_GATEWAY,
            "youtube_broadcast_discovery_failed",
            "active YouTube broadcasts could not be discovered",
            true,
        ),
    }
}

async fn select_youtube_broadcast(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let request = match parse_json_body(&headers, body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let Some(sources) = source_runtime_for(&state, &source_id) else {
        return source_request_unavailable(&source_id);
    };
    let SelectYouTubeBroadcastRequest { expected_revision, video_id } = request;
    match semantic_engine_source_runtime::select_youtube_broadcast(
        source_id,
        expected_revision,
        video_id,
        sources,
    )
    .await
    {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(_) => transport_error(
            StatusCode::CONFLICT,
            "youtube_broadcast_selection_failed",
            "the YouTube broadcast could not be selected",
            false,
        ),
    }
}

async fn start_twitch_source(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let request = match parse_json_body(&headers, body) {
        Ok(request) => request,
        Err(error) => return error.response(),
    };
    let Some(sources) = source_runtime_for(&state, &source_id) else {
        return source_request_unavailable(&source_id);
    };
    let StartSourceRequest { expected_revision, session_id } = request;
    match semantic_engine_source_runtime::start_source(
        source_id,
        expected_revision,
        session_id,
        sources,
    )
    .await
    {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(_) => transport_error(
            StatusCode::CONFLICT,
            "source_start_failed",
            "the source could not be started for this session",
            false,
        ),
    }
}

async fn pause_source(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let Some(sources) = source_runtime_for(&state, &source_id) else {
        return source_request_unavailable(&source_id);
    };
    match semantic_engine_source_runtime::stop_source(source_id, sources).await {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(_) => transport_error(
            StatusCode::CONFLICT,
            "source_pause_failed",
            "the source could not be paused",
            false,
        ),
    }
}

async fn delete_source(
    State(state): State<AppState>,
    Path(source_id): Path<String>,
    query: Result<Query<DeleteSourceQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Response {
    let _permit = match authorize_source_request(&state, &headers) {
        Ok(permit) => permit,
        Err(error) => return error.response(),
    };
    let Query(query) = match query {
        Ok(query) => query,
        Err(_) => {
            return transport_error(
                StatusCode::BAD_REQUEST,
                "invalid_source_query",
                "expected_revision is required",
                false,
            );
        }
    };
    let Some(sources) = source_runtime_for(&state, &source_id) else {
        return source_request_unavailable(&source_id);
    };
    match semantic_engine_source_runtime::delete_source(source_id, query.expected_revision, sources)
        .await
    {
        Ok(receipt) => json_response(StatusCode::OK, receipt),
        Err(_) => transport_error(
            StatusCode::CONFLICT,
            "source_delete_failed",
            "the paused source could not be deleted at this revision",
            false,
        ),
    }
}

async fn events_websocket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    query: Result<Query<EventsQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize_origin(&state, &headers) {
        return error.response();
    }
    if let Err(error) = take_quota(&state) {
        return error.response();
    }
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => {
            return transport_error(
                StatusCode::BAD_REQUEST,
                "invalid_events_query",
                "the WebSocket event query is malformed",
                false,
            );
        }
    };
    if query.session_id.is_empty() || query.limit == 0 || query.limit > 1_000 {
        return transport_error(
            StatusCode::BAD_REQUEST,
            "invalid_events_query",
            "session_id and a limit between 1 and 1000 are required",
            false,
        );
    }
    let protocols = websocket_protocols(&headers);
    if !protocols.contains(&WEBSOCKET_PROTOCOL) {
        return transport_error(
            StatusCode::UPGRADE_REQUIRED,
            "unsupported_protocol_version",
            "the semantic-engine.v2 WebSocket protocol is required",
            false,
        );
    }
    let authenticated = protocols.iter().any(|protocol| {
        protocol
            .strip_prefix(WEBSOCKET_TOKEN_PREFIX)
            .is_some_and(|token| token_matches(&state.token_hash, token))
    });
    if !authenticated {
        return transport_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid WebSocket token protocol is required",
            false,
        );
    }
    let Ok(connection_permit) = state.websocket_connections.clone().try_acquire_owned() else {
        return transport_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "websocket_capacity",
            "too many WebSocket clients are connected",
            true,
        );
    };
    ws.max_message_size(MAX_REQUEST_BYTES)
        .max_frame_size(MAX_REQUEST_BYTES)
        .protocols([WEBSOCKET_PROTOCOL])
        .on_upgrade(move |socket| stream_events(socket, state, query, connection_permit))
}

async fn stream_events(
    mut socket: axum::extract::ws::WebSocket,
    state: AppState,
    query: EventsQuery,
    _connection_permit: tokio::sync::OwnedSemaphorePermit,
) {
    let mut after_sequence = query.after_sequence;
    let mut request_number = 0_u64;
    let mut interval = time::interval(state.event_poll_interval);
    let mut shutdown = state.shutdown.clone();
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                request_number = request_number.saturating_add(1);
                let request = RequestEnvelope {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: format!("ws-{request_number}"),
                    command: Command::Events {
                        session_id: query.session_id.clone(),
                        after_sequence,
                        limit: query.limit,
                    },
                };
                let service = state.service.clone();
                let response = match tokio::task::spawn_blocking(move || {
                    handle(&mut service.blocking_lock(), request)
                }).await {
                    Ok(response) => response,
                    Err(_) => break,
                };
                let is_error = response.status == ResponseStatus::Error;
                let next_sequence = last_event_sequence(response.result.as_ref());
                if is_error || next_sequence.is_some() {
                    let Ok(payload) = serde_json::to_string(&response) else { break; };
                    if socket.send(Message::Text(payload.into())).await.is_err() { break; }
                }
                if is_error { break; }
                if let Some(sequence) = next_sequence {
                    after_sequence = sequence;
                }
            }
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() { break; }
                    }
                    Some(Ok(_)) => {}
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
        }
    }
}

fn validate_config(config: &LoopbackConfig) -> Result<(), LoopbackError> {
    if !config.bind_addr.ip().is_loopback() {
        return Err(LoopbackError::InvalidConfig(
            "the server may only bind to a loopback address".into(),
        ));
    }
    if config.allowed_origins.len() > 32
        || config.allowed_origins.iter().any(|origin| !valid_origin(origin))
        || !(1..=100_000).contains(&config.max_requests_per_second)
        || !(1..=1_024).contains(&config.max_in_flight)
        || !(1..=128).contains(&config.max_websocket_connections)
        || !(Duration::from_millis(10)..=Duration::from_secs(10))
            .contains(&config.event_poll_interval)
    {
        return Err(LoopbackError::InvalidConfig("one or more resource limits are invalid".into()));
    }
    Ok(())
}

fn valid_origin(origin: &str) -> bool {
    if origin.len() > 512 {
        return false;
    }
    let Ok(uri) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    matches!(uri.scheme_str(), Some("http" | "https" | "tauri"))
        && uri.authority().is_some()
        && !origin.ends_with('/')
        && uri.path_and_query().is_none_or(|path| path.as_str() == "/")
}

fn generate_token() -> Result<String, LoopbackError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| LoopbackError::Entropy(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn token_matches(expected: &[u8; 32], candidate: &str) -> bool {
    let candidate = token_hash(candidate);
    expected
        .iter()
        .zip(candidate)
        .fold(0_u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn authorize_http(state: &AppState, headers: &HeaderMap) -> Result<(), TransportFailure> {
    authorize_origin(state, headers)?;
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token_matches(&state.token_hash, token));
    if authorized {
        Ok(())
    } else {
        Err(TransportFailure {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "a valid bearer token is required",
            retryable: false,
        })
    }
}

fn authorize_source_request(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<tokio::sync::OwnedSemaphorePermit, TransportFailure> {
    authorize_http(state, headers)?;
    negotiate_http_version(headers)?;
    take_quota(state)?;
    state.in_flight.clone().try_acquire_owned().map_err(|_| TransportFailure {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "backpressure",
        message: "too many requests are already in flight",
        retryable: true,
    })
}

fn source_runtime_for<'a>(state: &'a AppState, source_id: &str) -> Option<&'a Arc<SourceRuntime>> {
    valid_source_id(source_id).then_some(state.sources.as_ref()).flatten()
}

fn source_request_unavailable(source_id: &str) -> Response {
    if !valid_source_id(source_id) {
        transport_error(
            StatusCode::BAD_REQUEST,
            "invalid_source_id",
            "the source identifier is invalid",
            false,
        )
    } else {
        source_runtime_unavailable()
    }
}

fn source_runtime_unavailable() -> Response {
    transport_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "source_runtime_unavailable",
        "input source management is not available in this host",
        false,
    )
}

fn valid_source_id(source_id: &str) -> bool {
    let bytes = source_id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn parse_json_body<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<T, TransportFailure> {
    if !is_json_content_type(headers) {
        return Err(TransportFailure {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code: "unsupported_media_type",
            message: "content-type must be application/json",
            retryable: false,
        });
    }
    let body = match body {
        Ok(body) => body,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return Err(TransportFailure {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                code: "request_too_large",
                message: "the request body exceeds the 1 MiB transport limit",
                retryable: false,
            });
        }
        Err(_) => {
            return Err(TransportFailure {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_request_body",
                message: "the request body could not be read",
                retryable: false,
            });
        }
    };
    serde_json::from_slice(&body).map_err(|_| TransportFailure {
        status: StatusCode::BAD_REQUEST,
        code: "invalid_json",
        message: "the request body is not valid JSON for this operation",
        retryable: false,
    })
}

fn json_response<T: Serialize>(status: StatusCode, value: T) -> Response {
    let mut response = (status, Json(value)).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn authorize_origin(state: &AppState, headers: &HeaderMap) -> Result<(), TransportFailure> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let allowed = origin
        .to_str()
        .ok()
        .is_some_and(|origin| state.allowed_origins.iter().any(|allowed| allowed == origin));
    if allowed {
        Ok(())
    } else {
        Err(TransportFailure {
            status: StatusCode::FORBIDDEN,
            code: "origin_forbidden",
            message: "the browser origin is not allowed",
            retryable: false,
        })
    }
}

fn negotiate_http_version(headers: &HeaderMap) -> Result<(), TransportFailure> {
    if headers
        .get(HTTP_PROTOCOL_HEADER)
        .is_some_and(|value| value.as_bytes() == PROTOCOL_VERSION.to_string().as_bytes())
    {
        return Ok(());
    }
    Err(TransportFailure {
        status: StatusCode::UPGRADE_REQUIRED,
        code: "unsupported_protocol_version",
        message: "x-semantic-engine-protocol must select version 2",
        retryable: false,
    })
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn take_quota(state: &AppState) -> Result<(), TransportFailure> {
    let now = Instant::now();
    let mut requests = state.request_quota.lock().unwrap_or_else(|error| error.into_inner());
    while requests
        .front()
        .is_some_and(|started| now.duration_since(*started) >= Duration::from_secs(1))
    {
        requests.pop_front();
    }
    if requests.len() >= state.max_requests_per_second {
        return Err(TransportFailure {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: "the local request quota has been reached",
            retryable: true,
        });
    }
    requests.push_back(now);
    Ok(())
}

fn websocket_protocols(headers: &HeaderMap) -> Vec<&str> {
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').map(str::trim).collect())
        .unwrap_or_default()
}

fn last_event_sequence(result: Option<&Value>) -> Option<u64> {
    result?.get("events")?.as_array()?.last()?.get("sequence")?.as_u64()
}

fn transport_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retryable: bool,
) -> Response {
    let mut response = (
        status,
        Json(TransportErrorEnvelope { error: TransportErrorBody { code, message, retryable } }),
    )
        .into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if retryable {
        response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

fn default_event_limit() -> usize {
    100
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refuses_non_loopback_bind_addresses() {
        let config = LoopbackConfig {
            bind_addr: "0.0.0.0:17831".parse().unwrap(),
            ..LoopbackConfig::default()
        };
        assert!(matches!(validate_config(&config), Err(LoopbackError::InvalidConfig(_))));
    }

    #[test]
    fn generated_tokens_are_high_entropy_and_compare_without_plaintext_storage() {
        let first = generate_token().unwrap();
        let second = generate_token().unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
        let hash = token_hash(&first);
        assert!(token_matches(&hash, &first));
        assert!(!token_matches(&hash, &second));
    }

    #[test]
    fn extracts_only_the_last_delivered_event_sequence() {
        let value = json!({"events": [{"sequence": 4}, {"sequence": 5}], "latest_sequence": 99});
        assert_eq!(last_event_sequence(Some(&value)), Some(5));
    }

    #[test]
    fn accepts_only_exact_web_origins_without_paths_or_unsupported_schemes() {
        assert!(valid_origin("http://localhost:5173"));
        assert!(valid_origin("tauri://localhost"));
        assert!(!valid_origin("http://localhost:5173/"));
        assert!(!valid_origin("http://localhost:5173/private"));
        assert!(!valid_origin("file:///tmp/app.html"));
        assert!(!valid_origin("not an origin"));
    }
}
