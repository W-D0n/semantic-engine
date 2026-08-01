use semantic_engine_core::{OperatorResolutionRequest, Submission};
use semantic_engine_service::{SemanticEngineService, ServiceError, StartSession};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_REQUEST_ID_CHARS: usize = 128;
pub const UNKNOWN_REQUEST_ID: &str = "unknown";

#[derive(Clone, Debug, Deserialize)]
pub struct RequestEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "command", content = "params", rename_all = "snake_case")]
pub enum Command {
    StartSession(StartSession),
    GetSession {
        session_id: String,
    },
    Submit {
        session_id: String,
        submission: Submission,
    },
    Resolve {
        session_id: String,
        request: OperatorResolutionRequest,
    },
    EndSession {
        session_id: String,
    },
    Events {
        session_id: String,
        #[serde(default)]
        after_sequence: u64,
        #[serde(default = "default_event_limit")]
        limit: usize,
    },
    Stats,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResponseEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Ok,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProtocolError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

pub fn handle_json_line(service: &mut SemanticEngineService, line: &[u8]) -> ResponseEnvelope {
    let request = match serde_json::from_slice::<RequestEnvelope>(line) {
        Ok(request) => request,
        Err(error) => {
            return failure(
                UNKNOWN_REQUEST_ID,
                "malformed_request",
                format!("request is not valid protocol JSON: {error}"),
                false,
            );
        }
    };
    handle(service, request)
}

pub fn handle(service: &mut SemanticEngineService, request: RequestEnvelope) -> ResponseEnvelope {
    if request.protocol_version != PROTOCOL_VERSION {
        return failure(
            safe_request_id(&request.request_id),
            "unsupported_protocol_version",
            format!(
                "protocol_version {} is not supported; expected {PROTOCOL_VERSION}",
                request.protocol_version
            ),
            false,
        );
    }
    if !valid_request_id(&request.request_id) {
        return failure(
            UNKNOWN_REQUEST_ID,
            "invalid_request_id",
            "request_id must contain 1 to 128 portable identifier characters".into(),
            false,
        );
    }

    let request_id = request.request_id;
    let result = match request.command {
        Command::StartSession(start) => service.start_session(start).and_then(to_value),
        Command::GetSession { session_id } => service.session(&session_id).and_then(to_value),
        Command::Submit { session_id, submission } => {
            service.submit(&session_id, submission).and_then(to_value)
        }
        Command::Resolve { session_id, request } => {
            service.resolve_session(&session_id, request).and_then(to_value)
        }
        Command::EndSession { session_id } => service.end_session(&session_id).and_then(to_value),
        Command::Events { session_id, after_sequence, limit } => {
            service.session_events(&session_id, after_sequence, limit).and_then(to_value)
        }
        Command::Stats => to_value(service.stats()),
    };

    match result {
        Ok(result) => success(request_id, result),
        Err(error) => service_failure(request_id, error),
    }
}

pub fn line_too_large_response() -> ResponseEnvelope {
    failure(
        UNKNOWN_REQUEST_ID,
        "request_too_large",
        "request line exceeds the transport limit".into(),
        false,
    )
}

fn default_event_limit() -> usize {
    100
}

fn valid_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.chars().count() <= MAX_REQUEST_ID_CHARS
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn safe_request_id(request_id: &str) -> &str {
    if valid_request_id(request_id) { request_id } else { UNKNOWN_REQUEST_ID }
}

fn to_value<T: Serialize>(value: T) -> Result<Value, ServiceError> {
    serde_json::to_value(value).map_err(|error| ServiceError::Internal(error.to_string()))
}

fn success(request_id: String, result: Value) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        status: ResponseStatus::Ok,
        result: Some(result),
        error: None,
    }
}

fn failure(
    request_id: impl Into<String>,
    code: &'static str,
    message: String,
    retryable: bool,
) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request_id.into(),
        status: ResponseStatus::Error,
        result: None,
        error: Some(ProtocolError { code, message, retryable }),
    }
}

fn service_failure(request_id: String, error: ServiceError) -> ResponseEnvelope {
    let (code, retryable) = match &error {
        ServiceError::InvalidConfig => ("invalid_service_config", false),
        ServiceError::IdentityConflict => ("identity_conflict", false),
        ServiceError::ValidationMissing => ("validation_missing", false),
        ServiceError::SessionConflict => ("session_conflict", false),
        ServiceError::SessionMissing => ("session_missing", false),
        ServiceError::SessionEnded => ("session_ended", false),
        ServiceError::SessionCapacityExceeded => ("session_capacity_exceeded", true),
        ServiceError::InvalidSession => ("invalid_session", false),
        ServiceError::Resolution(_) => ("resolution_rejected", false),
        ServiceError::Audit(_) => ("audit_unavailable", true),
        ServiceError::Internal(_) => ("internal_error", true),
    };
    failure(request_id, code, error.to_string(), retryable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_line() -> Vec<u8> {
        br#"{"protocol_version":1,"request_id":"req-1","command":"start_session","params":{"session_id":"live-1","round":{"id":"round-1","targets":[{"id":"elden-ring","canonical":"Elden Ring","aliases":["ER"]}],"policy":{"accept_threshold":0.87,"review_threshold":0.72,"ambiguity_margin":0.05}},"context_package_sha256":null}}"#.to_vec()
    }

    #[test]
    fn dispatches_a_session_lifecycle_and_preserves_correlation() {
        let mut service = SemanticEngineService::in_memory().unwrap();
        let started = handle_json_line(&mut service, &start_line());
        assert_eq!(started.status, ResponseStatus::Ok);
        assert_eq!(started.request_id, "req-1");

        let submitted = handle_json_line(
            &mut service,
            br#"{"protocol_version":1,"request_id":"req-2","command":"submit","params":{"session_id":"live-1","submission":{"message_id":"msg-1","participant_id":"viewer-7","source_sequence":1,"text":"eldern ring"}}}"#,
        );
        assert_eq!(submitted.status, ResponseStatus::Ok);
        assert_eq!(submitted.result.unwrap()["decision"], "accepted");
    }

    #[test]
    fn events_never_echo_raw_chat_text() {
        let mut service = SemanticEngineService::in_memory().unwrap();
        handle_json_line(&mut service, &start_line());
        handle_json_line(
            &mut service,
            br#"{"protocol_version":1,"request_id":"req-2","command":"submit","params":{"session_id":"live-1","submission":{"message_id":"msg-1","participant_id":"viewer-7","source_sequence":1,"text":"PRIVATE CHAT TEXT"}}}"#,
        );
        let events = handle_json_line(
            &mut service,
            br#"{"protocol_version":1,"request_id":"req-3","command":"events","params":{"session_id":"live-1","after_sequence":0,"limit":100}}"#,
        );
        let encoded = serde_json::to_string(&events).unwrap();
        assert!(!encoded.contains("PRIVATE CHAT TEXT"));
        assert!(!encoded.contains("matched_expression"));
    }

    #[test]
    fn rejects_malformed_and_future_protocol_requests_with_stable_codes() {
        let mut service = SemanticEngineService::in_memory().unwrap();
        let malformed = handle_json_line(&mut service, b"not-json");
        assert_eq!(malformed.error.unwrap().code, "malformed_request");
        let future = handle_json_line(
            &mut service,
            br#"{"protocol_version":2,"request_id":"future-1","command":"stats"}"#,
        );
        assert_eq!(future.error.unwrap().code, "unsupported_protocol_version");
    }

    #[test]
    fn all_public_session_and_protocol_schemas_are_valid_json() {
        let schemas = [
            include_str!("../../../contracts/round.schema.json"),
            include_str!("../../../contracts/session-start.schema.json"),
            include_str!("../../../contracts/session.schema.json"),
            include_str!("../../../contracts/session-event.schema.json"),
            include_str!("../../../contracts/session-events-page.schema.json"),
            include_str!("../../../contracts/operator-resolution-request.schema.json"),
            include_str!("../../../contracts/protocol-request.schema.json"),
            include_str!("../../../contracts/protocol-response.schema.json"),
        ];
        for schema in schemas {
            let parsed: Value = serde_json::from_str(schema).expect("schema must be valid JSON");
            assert_eq!(parsed["$schema"], "https://json-schema.org/draft/2020-12/schema");
        }
    }
}
