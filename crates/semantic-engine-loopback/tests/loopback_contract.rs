use std::{net::SocketAddr, sync::Arc, time::Duration};

use futures_util::StreamExt;
use semantic_engine_loopback::{
    HTTP_PROTOCOL_HEADER, LoopbackConfig, WEBSOCKET_PROTOCOL, start, start_shared_with_sources,
};
use semantic_engine_service::SemanticEngineService;
use semantic_engine_source_runtime::SourceRuntime;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};

#[tokio::test]
async fn http_transport_enforces_auth_origin_and_version_then_dispatches() {
    let server = test_server(100).await;
    let command = start_command();

    let unauthorized = http_post(server.addr(), &command, &[]).await;
    assert_eq!(unauthorized.status, 401);
    assert_eq!(unauthorized.json()["error"]["code"], "unauthorized");

    let forbidden = http_post(
        server.addr(),
        &command,
        &[
            ("Authorization", format!("Bearer {}", server.token())),
            (HTTP_PROTOCOL_HEADER, "1".into()),
            ("Origin", "https://attacker.example".into()),
        ],
    )
    .await;
    assert_eq!(forbidden.status, 403);
    assert_eq!(forbidden.json()["error"]["code"], "origin_forbidden");

    let missing_version = http_post(
        server.addr(),
        &command,
        &[("Authorization", format!("Bearer {}", server.token()))],
    )
    .await;
    assert_eq!(missing_version.status, 426);

    let unsupported_media = http_post(
        server.addr(),
        &command,
        &[
            ("Authorization", format!("Bearer {}", server.token())),
            (HTTP_PROTOCOL_HEADER, "1".into()),
            ("Origin", "http://localhost".into()),
        ],
    )
    .await;
    assert_eq!(unsupported_media.status, 415);
    assert_eq!(unsupported_media.json()["error"]["code"], "unsupported_media_type");

    let oversized = authorized_post(
        server.addr(),
        server.token(),
        &"x".repeat(semantic_engine_loopback::MAX_REQUEST_BYTES + 1),
    )
    .await;
    assert_eq!(oversized.status, 413);
    assert_eq!(oversized.json()["error"]["code"], "request_too_large");

    let accepted = authorized_post(server.addr(), server.token(), &command).await;
    assert_eq!(accepted.status, 200);
    let payload = accepted.json();
    assert_eq!(payload["protocol_version"], 1);
    assert_eq!(payload["request_id"], "http-start");
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["result"]["state"], "active");
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn websocket_uses_one_authenticated_subprotocol_and_streams_protocol_events() {
    let server = test_server(100).await;
    let started = authorized_post(server.addr(), server.token(), &start_command()).await;
    assert_eq!(started.status, 200);

    let url = format!(
        "ws://{}/v1/events/ws?session_id=loopback-session&after_sequence=0&limit=100",
        server.addr()
    );
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert("origin", HeaderValue::from_static("http://localhost"));
    request.headers_mut().insert(
        "sec-websocket-protocol",
        HeaderValue::from_str(&format!(
            "{WEBSOCKET_PROTOCOL}, semantic-engine.token.{}",
            server.token()
        ))
        .unwrap(),
    );
    let (mut socket, response) = connect_async(request).await.unwrap();
    assert_eq!(response.headers().get("sec-websocket-protocol").unwrap(), WEBSOCKET_PROTOCOL);
    let message = timeout(Duration::from_secs(2), socket.next()).await.unwrap().unwrap().unwrap();
    let Message::Text(payload) = message else {
        panic!("expected a text protocol response");
    };
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["result"]["events"][0]["type"], "session_started");
    assert!(!payload.to_string().contains("matched_expression"));
    timeout(Duration::from_secs(2), server.shutdown()).await.unwrap().unwrap();
    let closed = timeout(Duration::from_secs(2), socket.next()).await.unwrap();
    assert!(matches!(closed, None | Some(Err(_)) | Some(Ok(Message::Close(_)))));
}

#[tokio::test]
async fn quota_rejects_bursts_without_queuing_them() {
    let server = test_server(1).await;
    let first = authorized_post(server.addr(), server.token(), &start_command()).await;
    assert_eq!(first.status, 200);
    let second = authorized_post(
        server.addr(),
        server.token(),
        &json!({"protocol_version": 1, "request_id": "stats", "command": "stats"}).to_string(),
    )
    .await;
    assert_eq!(second.status, 429);
    assert_eq!(second.json()["error"]["code"], "rate_limited");
    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_api_is_authenticated_and_never_returns_platform_tokens() {
    let temporary = tempfile::tempdir().unwrap();
    let service = Arc::new(tokio::sync::Mutex::new(SemanticEngineService::in_memory().unwrap()));
    let sources = Arc::new(
        SourceRuntime::open(temporary.path().join("sources.sqlite3"), service.clone()).unwrap(),
    );
    let server = start_shared_with_sources(
        service,
        sources,
        LoopbackConfig {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            allowed_origins: vec!["http://localhost".into()],
            ..LoopbackConfig::default()
        },
    )
    .await
    .unwrap();

    let unauthorized = http_request(server.addr(), "GET", "/v1/sources", "", &[]).await;
    assert_eq!(unauthorized.status, 401);

    let created = authorized_request(
        server.addr(),
        server.token(),
        "POST",
        "/v1/sources/twitch",
        &json!({"display_name": "Canal pilote", "client_id": "publicclient123"}).to_string(),
        true,
    )
    .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let created_json = created.json();
    let source_id = created_json["source_id"].as_str().unwrap();
    let revision = created_json["revision"].as_u64().unwrap();
    assert_eq!(created_json["authenticated"], false);
    assert_eq!(created_json["credential_id"], Value::Null);
    assert!(!created.body.contains("access_token"));
    assert!(!created.body.contains("refresh_token"));

    let youtube = authorized_request(
        server.addr(),
        server.token(),
        "POST",
        "/v1/sources/youtube",
        &json!({
            "display_name": "Live pilote",
            "client_id": "123.apps.googleusercontent.com",
            "policy_acknowledged": true
        })
        .to_string(),
        true,
    )
    .await;
    assert_eq!(youtube.status, 201, "{}", youtube.body);
    let youtube_json = youtube.json();
    let youtube_id = youtube_json["source_id"].as_str().unwrap();
    let youtube_revision = youtube_json["revision"].as_u64().unwrap();
    assert_eq!(youtube_json["adapter"], "youtube-live-chat");
    assert_eq!(youtube_json["authenticated"], false);
    assert_eq!(youtube_json["settings"]["video_id"], "");
    assert!(!youtube.body.contains("access_token"));
    assert!(!youtube.body.contains("refresh_token"));

    let authorization = authorized_request(
        server.addr(),
        server.token(),
        "POST",
        &format!("/v1/sources/{youtube_id}/authorization"),
        "",
        false,
    )
    .await;
    assert!(matches!(authorization.status, 200 | 502), "{}", authorization.body);
    if authorization.status == 200 {
        assert!(
            authorization.json()["authorization_uri"]
                .as_str()
                .is_some_and(|value| value.starts_with("https://accounts.google.com/"))
        );
    }

    let tested = authorized_request(
        server.addr(),
        server.token(),
        "POST",
        &format!("/v1/sources/{youtube_id}/test"),
        "",
        false,
    )
    .await;
    assert_eq!(tested.status, 502, "{}", tested.body);

    let broadcasts = authorized_request(
        server.addr(),
        server.token(),
        "GET",
        &format!("/v1/sources/{youtube_id}/youtube/broadcasts"),
        "",
        false,
    )
    .await;
    assert_eq!(broadcasts.status, 502, "{}", broadcasts.body);
    assert_eq!(broadcasts.json()["error"]["code"], "youtube_broadcast_discovery_failed");

    let started = authorized_request(
        server.addr(),
        server.token(),
        "POST",
        &format!("/v1/sources/{youtube_id}/start"),
        &json!({"expected_revision": youtube_revision, "session_id": "session-1"}).to_string(),
        true,
    )
    .await;
    assert_eq!(started.status, 409, "{}", started.body);
    assert_eq!(started.json()["error"]["code"], "source_start_failed");

    let listed =
        authorized_request(server.addr(), server.token(), "GET", "/v1/sources", "", false).await;
    assert_eq!(listed.status, 200);
    assert_eq!(listed.json().as_array().unwrap().len(), 2);

    let deleted = authorized_request(
        server.addr(),
        server.token(),
        "DELETE",
        &format!("/v1/sources/{source_id}?expected_revision={revision}"),
        "",
        false,
    )
    .await;
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert_eq!(deleted.json()["credential_purged"], true);
    assert_eq!(deleted.json()["durable_source_purged"], true);

    let deleted_youtube = authorized_request(
        server.addr(),
        server.token(),
        "DELETE",
        &format!("/v1/sources/{youtube_id}?expected_revision={youtube_revision}"),
        "",
        false,
    )
    .await;
    assert_eq!(deleted_youtube.status, 200, "{}", deleted_youtube.body);
    assert_eq!(deleted_youtube.json()["provider_revocation"], "not_applicable");

    let listed =
        authorized_request(server.addr(), server.token(), "GET", "/v1/sources", "", false).await;
    assert_eq!(listed.json(), json!([]));
    server.shutdown().await.unwrap();
}

async fn test_server(max_requests_per_second: usize) -> semantic_engine_loopback::LoopbackServer {
    let service = SemanticEngineService::in_memory().unwrap();
    start(
        service,
        LoopbackConfig {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            allowed_origins: vec!["http://localhost".into()],
            max_requests_per_second,
            ..LoopbackConfig::default()
        },
    )
    .await
    .unwrap()
}

fn start_command() -> String {
    json!({
        "protocol_version": 1,
        "request_id": "http-start",
        "command": "start_session",
        "params": {
            "session_id": "loopback-session",
            "round": {
                "id": "loopback-round",
                "targets": [{"id": "elden-ring", "canonical": "Elden Ring", "aliases": ["ER"]}],
                "policy": {"accept_threshold": 0.87, "review_threshold": 0.72, "ambiguity_margin": 0.05}
            },
            "context_package_sha256": null
        }
    })
    .to_string()
}

async fn authorized_post(addr: SocketAddr, token: &str, body: &str) -> HttpResponse {
    http_post(
        addr,
        body,
        &[
            ("Authorization", format!("Bearer {token}")),
            ("Content-Type", "application/json".into()),
            (HTTP_PROTOCOL_HEADER, "1".into()),
            ("Origin", "http://localhost".into()),
        ],
    )
    .await
}

struct HttpResponse {
    status: u16,
    body: String,
}

impl HttpResponse {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

async fn http_post(addr: SocketAddr, body: &str, headers: &[(&str, String)]) -> HttpResponse {
    http_request(addr, "POST", "/v1/commands", body, headers).await
}

async fn authorized_request(
    addr: SocketAddr,
    token: &str,
    method: &str,
    path: &str,
    body: &str,
    json_body: bool,
) -> HttpResponse {
    let mut headers = vec![
        ("Authorization", format!("Bearer {token}")),
        (HTTP_PROTOCOL_HEADER, "1".into()),
        ("Origin", "http://localhost".into()),
    ];
    if json_body {
        headers.push(("Content-Type", "application/json".into()));
    }
    http_request(addr, method, path, body, &headers).await
}

async fn http_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: &str,
    headers: &[(&str, String)],
) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    let (head, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("invalid HTTP response: {response:?}"));
    let status = head.lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    HttpResponse { status, body: body.into() }
}
