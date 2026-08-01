use std::{net::SocketAddr, time::Duration};

use futures_util::StreamExt;
use semantic_engine_loopback::{HTTP_PROTOCOL_HEADER, LoopbackConfig, WEBSOCKET_PROTOCOL, start};
use semantic_engine_service::SemanticEngineService;
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
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut request = format!(
        "POST /v1/commands HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n",
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
