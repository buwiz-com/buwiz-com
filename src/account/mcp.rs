//! `POST /mcp`: the MCP endpoint for AI agents (Streamable HTTP, JSON
//! answers only, no sessions).
//!
//! Every request needs an access token. Without a valid one the answer is
//! 401 with a `WWW-Authenticate` header naming the RFC 9728 metadata, which
//! is how MCP clients find where to sign in. Only an agent's token (scope
//! `mcp`) is accepted: an app's token answers 403 `insufficient_scope`, so
//! every agent shows on the account page and can be removed there. All
//! tools only read; filing stays in the Buwiz app.

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::account::agents::request_origin;
use crate::account::claims;
use crate::account::oauth::{Bearer, SCOPE_MCP};
use crate::base::error::ApiError;
use crate::base::state::AppState;
use crate::filing::submissions;

const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

/// JSON-RPC error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    bearer: Result<Bearer, ApiError>,
    body: String,
) -> Response {
    // Browsers must not drive this endpoint from other sites (DNS rebinding).
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok())
        && !state.config.is_own_origin(origin)
    {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    let bearer = match bearer {
        Ok(bearer) => bearer,
        Err(ApiError::InvalidToken) => return challenge(&state, &headers, StatusCode::UNAUTHORIZED, "invalid_token"),
        Err(error) => return error.into_response(),
    };
    if bearer.scope != SCOPE_MCP {
        return challenge(&state, &headers, StatusCode::FORBIDDEN, "insufficient_scope");
    }
    let message: Value = match serde_json::from_str(&body) {
        Ok(message) => message,
        Err(_) => return rpc_error(Value::Null, PARSE_ERROR, "not JSON"),
    };
    if message.is_array() {
        return rpc_error(Value::Null, INVALID_REQUEST, "JSON-RPC batches are not supported");
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response from the client to a request we never send.
        return StatusCode::ACCEPTED.into_response();
    };
    let Some(id) = message.get("id").cloned() else {
        // Notifications (e.g. notifications/initialized) need no answer.
        return StatusCode::ACCEPTED.into_response();
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(&state, &bearer, &params).await,
        _ => Err((METHOD_NOT_FOUND, format!("unknown method {method}"))),
    };
    match result {
        Ok(result) => Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response(),
        Err((code, message)) => rpc_error(id, code, &message),
    }
}

/// A refusal with the RFC 6750 `WWW-Authenticate` challenge that sends the
/// client to the RFC 9728 metadata to sign in again.
fn challenge(state: &AppState, headers: &HeaderMap, status: StatusCode, error: &str) -> Response {
    let origin = request_origin(&state.config, headers);
    let challenge = format!(
        r#"Bearer error="{error}", resource_metadata="{origin}/.well-known/oauth-protected-resource/mcp", scope="{SCOPE_MCP}""#
    );
    let mut response = (status, Json(json!({ "error": error }))).into_response();
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    let body = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } });
    Json(body).into_response()
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested.filter(|version| PROTOCOL_VERSIONS.contains(version)).unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "buwiz", "title": "Buwiz", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Read the signed-in person's Buwiz account: active TINs (tax profiles), TIN claims \
            waiting for review, and returns sent to BIR. These tools only read. Filing happens in the Buwiz app.",
    })
}

fn tools() -> Value {
    let no_input = json!({ "type": "object", "properties": {}, "additionalProperties": false });
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    json!([
        {
            "name": "list_tax_profiles",
            "title": "Active TINs",
            "description": "The person's active TINs (tax profiles) and what each allows in the Buwiz app.",
            "inputSchema": no_input,
            "annotations": read_only,
        },
        {
            "name": "list_tin_claims",
            "title": "TIN claims",
            "description": "Every TIN the person has added or asked to activate, with its review status, why a claim was sent back, and the reviewer's note on a rejection.",
            "inputSchema": no_input,
            "annotations": read_only,
        },
        {
            "name": "list_submissions",
            "title": "Returns sent",
            "description": "The 50 newest returns sent to BIR through Buwiz: form, period, status and when it was sent.",
            "inputSchema": no_input,
            "annotations": read_only,
        },
        {
            "name": "get_submission",
            "title": "One return",
            "description": "One return sent through Buwiz, by id, including any error and its signed receipt.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string", "format": "uuid", "description": "Submission id" } },
                "required": ["id"],
                "additionalProperties": false,
            },
            "annotations": read_only,
        },
    ])
}

async fn call_tool(state: &AppState, bearer: &Bearer, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "name is required".to_owned()))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let account_id = bearer.account.id;
    let internal = |error: sqlx::Error| {
        tracing::error!(error = ?error, "mcp tool failed");
        (INTERNAL_ERROR, "something went wrong".to_owned())
    };
    let content = match name {
        "list_tax_profiles" => {
            json!({ "profiles": claims::active_profiles(&state.db, account_id, false).await.map_err(internal)? })
        }
        "list_tin_claims" => {
            let claims = claims::for_account(&state.db, account_id).await.map_err(internal)?;
            let claims: Vec<Value> = claims
                .into_iter()
                .map(|claim| {
                    // Staff's own words reach an agent only as the reason
                    // for a rejection, which the claimant reads on My TINs
                    // too; a claim sent back shows its fixed reasons.
                    let status =
                        if claim.status == "approved" && claim.paused_at.is_some() { "paused" } else { &claim.status };
                    let mut row = json!({
                        "tin_masked": claim.tin().masked(),
                        "registered_name": claim.registered_name,
                        "status": status,
                        "note": if claim.status == "rejected" { claim.decision_note.clone() } else { None },
                        "sent_at": claim.created_at.unix_timestamp(),
                    });
                    if claim.status == "needs_fix" {
                        let reasons: Vec<&str> = claim
                            .fix_reasons
                            .iter()
                            .filter_map(|code| claims::fix_reason(code))
                            .map(|(_, text)| text)
                            .collect();
                        row["reasons"] = json!(reasons);
                    }
                    row
                })
                .collect();
            json!({ "claims": claims })
        }
        "list_submissions" => {
            let views = submissions::recent(&state.db, account_id).await.map_err(internal)?;
            let views: Vec<_> = views.into_iter().map(|view| view.for_bearer(bearer)).collect();
            json!({ "submissions": views })
        }
        "get_submission" => {
            let Some(id) = arguments.get("id").and_then(Value::as_str).and_then(|raw| raw.parse::<Uuid>().ok()) else {
                return Ok(tool_error("id must be a submission id (UUID)"));
            };
            match submissions::find(&state.db, account_id, id).await.map_err(internal)? {
                Some(view) => serde_json::to_value(view.for_bearer(bearer))
                    .map_err(|_| (INTERNAL_ERROR, "encoding".to_owned()))?,
                None => return Ok(tool_error("no submission with that id on this account")),
            }
        }
        _ => return Err((INVALID_PARAMS, format!("unknown tool {name}"))),
    };
    Ok(json!({
        "content": [{ "type": "text", "text": content.to_string() }],
        "structuredContent": content,
        "isError": false,
    }))
}

/// A tool-level failure the model should see and can recover from.
fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_a_known_version_and_falls_back_to_the_newest() {
        assert_eq!(initialize(&json!({ "protocolVersion": "2025-06-18" }))["protocolVersion"], "2025-06-18");
        assert_eq!(initialize(&json!({ "protocolVersion": "1999-01-01" }))["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn every_tool_is_read_only_and_has_a_schema() {
        let tools = tools();
        for tool in tools.as_array().unwrap() {
            assert_eq!(tool["annotations"]["readOnlyHint"], true, "{}", tool["name"]);
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }
}
