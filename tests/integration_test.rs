use curl::easy::{Easy, List};
use redis::Commands;
use serde_json::{json, Value};
use std::env;
use std::io::Read;
use std::sync::{Arc, Barrier};
use std::thread;
use uuid::Uuid;

/// Helper to get the base URL for the wallet bridge service
fn get_base_url() -> String {
    env::var("WALLET_BRIDGE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string())
}

fn redis_connection() -> redis::Connection {
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
    redis::Client::open(redis_url)
        .expect("valid Redis URL")
        .get_connection()
        .expect("connect to integration-test Redis")
}

fn flow_key(request_id: &str) -> String {
    format!("flow:{request_id}")
}

fn flow_metadata(request_id: &str) -> Option<Value> {
    let raw: Option<String> = redis_connection()
        .get(flow_key(request_id))
        .expect("read flow metadata");
    raw.map(|raw| serde_json::from_str(&raw).expect("flow metadata is valid JSON"))
}

fn redis_ttl(key: &str) -> i64 {
    redis_connection().ttl(key).expect("read Redis TTL")
}

/// Helper to perform a GET request and return (status_code, body)
fn http_get(url: &str) -> (u32, String) {
    http_get_with_headers(url, &[])
}

fn http_get_with_headers(url: &str, headers: &[&str]) -> (u32, String) {
    let mut easy = Easy::new();
    easy.url(url).unwrap();

    if !headers.is_empty() {
        let mut header_list = List::new();
        for header in headers {
            header_list.append(header).unwrap();
        }
        easy.http_headers(header_list).unwrap();
    }

    let mut response_body = Vec::new();
    {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                response_body.extend_from_slice(data);
                Ok(data.len())
            })
            .unwrap();
        transfer.perform().unwrap();
    }

    let status_code = easy.response_code().unwrap();
    let body = String::from_utf8(response_body).unwrap_or_default();
    (status_code, body)
}

/// Helper to perform a POST request with JSON body and return (status_code, body)
fn http_post(url: &str, json_body: &Value) -> (u32, String) {
    let mut easy = Easy::new();
    easy.url(url).unwrap();
    easy.post(true).unwrap();

    let json_string = json_body.to_string();
    let mut json_bytes = json_string.as_bytes();
    easy.post_field_size(json_bytes.len() as u64).unwrap();

    let mut headers = List::new();
    headers.append("Content-Type: application/json").unwrap();
    easy.http_headers(headers).unwrap();

    let mut response_body = Vec::new();
    {
        let mut transfer = easy.transfer();
        transfer
            .read_function(|buf| Ok(json_bytes.read(buf).unwrap_or(0)))
            .unwrap();
        transfer
            .write_function(|data| {
                response_body.extend_from_slice(data);
                Ok(data.len())
            })
            .unwrap();
        transfer.perform().unwrap();
    }

    let status_code = easy.response_code().unwrap();
    let body = String::from_utf8(response_body).unwrap_or_default();
    (status_code, body)
}

/// Helper to perform a PUT request with JSON body and return (status_code, body)
fn http_put(url: &str, json_body: &Value) -> (u32, String) {
    let mut easy = Easy::new();
    easy.url(url).unwrap();
    easy.put(true).unwrap();

    let json_string = json_body.to_string();
    let json_len = json_string.len();
    let mut json_bytes = json_string.as_bytes();
    easy.in_filesize(json_len as u64).unwrap();

    let mut headers = List::new();
    headers.append("Content-Type: application/json").unwrap();
    easy.http_headers(headers).unwrap();

    let mut response_body = Vec::new();
    {
        let mut transfer = easy.transfer();
        transfer
            .read_function(|buf| Ok(json_bytes.read(buf).unwrap_or(0)))
            .unwrap();
        transfer
            .write_function(|data| {
                response_body.extend_from_slice(data);
                Ok(data.len())
            })
            .unwrap();
        transfer.perform().unwrap();
    }

    let status_code = easy.response_code().unwrap();
    let body = String::from_utf8(response_body).unwrap_or_default();
    (status_code, body)
}

/// Test the root endpoint returns service information
#[test]
fn test_root_endpoint() {
    let base_url = get_base_url();
    let (status_code, body) = http_get(&base_url);

    assert_eq!(status_code, 200);

    let json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    assert!(json.get("repo_url").is_some());
    assert!(json.get("version").is_some());
}

/// Test POST /request creates a new request and returns a request_id
#[test]
fn test_create_request() {
    let base_url = get_base_url();
    let payload = json!({
        "iv": "test_iv",
        "payload": "test_payload"
    });

    let url = format!("{}/request", base_url);
    let (status_code, body) = http_post(&url, &payload);

    assert_eq!(status_code, 200);

    let json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = json.get("request_id").expect("Missing request_id");
    assert!(request_id.is_string());
    assert!(!request_id.as_str().unwrap().is_empty());
}

/// Test GET /request/:id retrieves the request (one-time use)
#[test]
fn test_get_request_one_time_use() {
    let base_url = get_base_url();

    // Create a request
    let payload = json!({
        "iv": "get_test_iv",
        "payload": "get_test_payload"
    });

    let create_url = format!("{}/request", base_url);
    let (status_code, body) = http_post(&create_url, &payload);
    assert_eq!(status_code, 200);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // First GET should succeed
    let get_url = format!("{}/request/{}", base_url, request_id);
    let (first_status, first_body) = http_get(&get_url);

    assert_eq!(first_status, 200);

    let first_json: Value = serde_json::from_str(&first_body).expect("Failed to parse JSON");
    assert_eq!(first_json["iv"], "get_test_iv");
    assert_eq!(first_json["payload"], "get_test_payload");

    // Second GET should fail (one-time use)
    let (second_status, _) = http_get(&get_url);
    assert_eq!(second_status, 404);
}

/// Test PUT /response/:id stores a response for a request
#[test]
fn test_put_response_for_request() {
    let base_url = get_base_url();

    // Create a request
    let request_payload = json!({
        "iv": "request_iv",
        "payload": "request_payload"
    });

    let create_url = format!("{}/request", base_url);
    let (status_code, body) = http_post(&create_url, &request_payload);
    assert_eq!(status_code, 200);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // Put a response
    let response_payload = json!({
        "iv": "response_iv",
        "payload": "response_payload"
    });

    let put_url = format!("{}/response/{}", base_url, request_id);
    let (put_status, _) = http_put(&put_url, &response_payload);

    assert_eq!(put_status, 201);
}

/// Test GET /response/:id retrieves the response
#[test]
fn test_get_response() {
    let base_url = get_base_url();

    // Create a request
    let request_payload = json!({
        "iv": "req_iv",
        "payload": "req_payload"
    });

    let create_url = format!("{}/request", base_url);
    let (status_code, body) = http_post(&create_url, &request_payload);
    assert_eq!(status_code, 200);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // Put a response
    let response_payload = json!({
        "iv": "resp_iv",
        "payload": "resp_payload"
    });

    let put_url = format!("{}/response/{}", base_url, request_id);
    let (put_status, _) = http_put(&put_url, &response_payload);
    assert_eq!(put_status, 201);

    // Get the response
    let get_url = format!("{}/response/{}", base_url, request_id);
    let (get_status, get_body) = http_get(&get_url);

    assert_eq!(get_status, 200);

    let json: Value = serde_json::from_str(&get_body).expect("Failed to parse JSON");
    assert_eq!(json["status"], "completed");
    assert_eq!(json["response"]["iv"], "resp_iv");
    assert_eq!(json["response"]["payload"], "resp_payload");
}

/// Test GET /response/:id returns pending status when response not yet submitted
#[test]
fn test_response_pending_status() {
    let base_url = get_base_url();

    // Create a request
    let request_payload = json!({
        "iv": "pending_iv",
        "payload": "pending_payload"
    });

    let create_url = format!("{}/request", base_url);
    let (status_code, body) = http_post(&create_url, &request_payload);
    assert_eq!(status_code, 200);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // Get response before it's been PUT (should show pending)
    let get_url = format!("{}/response/{}", base_url, request_id);
    let (get_status, get_body) = http_get(&get_url);

    assert_eq!(get_status, 200);

    let json: Value = serde_json::from_str(&get_body).expect("Failed to parse JSON");
    let status = json["status"].as_str().unwrap();
    assert!(status == "initialized" || status == "retrieved" || status == "pending");
}

/// Test POST /response creates a standalone response
#[test]
fn test_create_standalone_response() {
    let base_url = get_base_url();

    let payload = json!({
        "iv": "standalone_iv",
        "payload": "standalone_payload"
    });

    let url = format!("{}/response", base_url);
    let (status_code, body) = http_post(&url, &payload);

    assert_eq!(status_code, 201);

    let json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = json.get("request_id").expect("Missing request_id");
    assert!(request_id.is_string());
}

/// Test standalone response flow: POST /response then GET /response/:id
#[test]
fn test_standalone_response_flow() {
    let base_url = get_base_url();

    // Create standalone response
    let payload = json!({
        "iv": "standalone_flow_iv",
        "payload": "standalone_flow_payload"
    });

    let create_url = format!("{}/response", base_url);
    let (status_code, body) = http_post(&create_url, &payload);
    assert_eq!(status_code, 201);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // Retrieve the response
    let get_url = format!("{}/response/{}", base_url, request_id);
    let (get_status, get_body) = http_get(&get_url);

    assert_eq!(get_status, 200);

    let json: Value = serde_json::from_str(&get_body).expect("Failed to parse JSON");
    assert_eq!(json["status"], "completed");
    assert_eq!(json["response"]["iv"], "standalone_flow_iv");
    assert_eq!(json["response"]["payload"], "standalone_flow_payload");
}

/// Test invalid request ID returns 404
#[test]
fn test_invalid_request_id() {
    let base_url = get_base_url();
    let url = format!("{}/request/00000000-0000-0000-0000-000000000000", base_url);
    let (status_code, _) = http_get(&url);

    assert_eq!(status_code, 404);
}

/// Test invalid response ID returns appropriate error
#[test]
fn test_invalid_response_id() {
    let base_url = get_base_url();
    let url = format!("{}/response/00000000-0000-0000-0000-000000000000", base_url);
    let (status_code, _) = http_get(&url);

    // Should return 404 or an error status
    assert!(status_code == 404 || (status_code >= 400 && status_code < 500));
}

/// Test that request requires both iv and payload fields
#[test]
fn test_create_request_validation() {
    let base_url = get_base_url();

    // Missing payload field
    let invalid_payload = json!({
        "iv": "test_iv"
    });

    let url = format!("{}/request", base_url);
    let (status_code, _) = http_post(&url, &invalid_payload);

    assert!(status_code >= 400 && status_code < 500);
}

// ---------------------------------------------------------------------------
// Client-supplied request_id (POST /request optional `request_id` field).
// Lets the RP address requests by any opaque ID it chooses (e.g. an HKDF
// output) so the bridge can be used as a generic content-addressable
// single-use store without the bridge needing to know the RP's key-derivation
// scheme.
// ---------------------------------------------------------------------------

fn fresh_id() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[test]
fn test_create_request_with_client_supplied_id() {
    let base_url = get_base_url();
    let id = fresh_id();
    let body = json!({
        "request_id": id,
        "iv": "iv-for-custom-id",
        "payload": "payload-for-custom-id",
    });
    let (s, b) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 200, "POST /request with custom id should succeed: {b}");
    let v: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(v["request_id"], id, "response echoes the supplied id");

    let (gs, gb) = http_get(&format!("{base_url}/request/{id}"));
    assert_eq!(gs, 200, "GET /request/<custom> should retrieve the payload");
    let gv: Value = serde_json::from_str(&gb).unwrap();
    assert_eq!(gv["iv"], "iv-for-custom-id");
    assert_eq!(gv["payload"], "payload-for-custom-id");

    // GETDEL semantics — second GET 404s.
    let (gs2, _) = http_get(&format!("{base_url}/request/{id}"));
    assert_eq!(gs2, 404);
}

#[test]
fn test_create_request_with_duplicate_id_returns_409() {
    let base_url = get_base_url();
    let id = fresh_id();
    let body = json!({"request_id": id, "iv": "x", "payload": "y"});

    let (s1, _) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s1, 200);

    let (s2, _) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s2, 409, "second POST with same request_id must 409");
}

// ---------------------------------------------------------------------------
// Server-driven URL overrides. The bridge takes no `app_id` input — it returns
// the whole `app_overrides` map blindly and the SDK picks its own entry.
//
// These tests require the bridge to be started with this exact fixture in the
// `APP_URL_OVERRIDES` env var (CI and the README do this):
//
//   {"app_integration_override_fixture":
//      {"app_clip_bundle_id":"org.example.integration.Clip",
//       "verify_url":"https://world.org/verify"}}
// ---------------------------------------------------------------------------

const FIXTURE_APP_ID: &str = "app_integration_override_fixture";
const FIXTURE_APP_CLIP_BUNDLE_ID: &str = "org.example.integration.Clip";
const FIXTURE_VERIFY_URL: &str = "https://world.org/verify";

#[test]
fn test_response_includes_full_override_map() {
    let base_url = get_base_url();
    // Opt in — the bridge only returns overrides to clients that advertise support.
    let body = json!({"iv": "x", "payload": "y", "supports_app_overrides": true});

    let (s, b) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 200, "POST /request should succeed: {b}");

    let v: Value = serde_json::from_str(&b).unwrap();
    let entry = v
        .get("app_overrides")
        .and_then(|m| m.get(FIXTURE_APP_ID))
        .unwrap_or_else(|| panic!("app_overrides map must contain the fixture entry. Is APP_URL_OVERRIDES set to the test fixture? Got: {b}"));

    assert_eq!(
        entry.get("app_clip_bundle_id").and_then(Value::as_str),
        Some(FIXTURE_APP_CLIP_BUNDLE_ID),
        "fixture entry must carry the configured app_clip_bundle_id: {b}"
    );
    assert_eq!(
        entry.get("verify_url").and_then(Value::as_str),
        Some(FIXTURE_VERIFY_URL),
        "fixture entry must carry the configured verify_url: {b}"
    );
}

#[test]
fn test_request_id_still_present_alongside_overrides() {
    // The override map is additive — the core request_id contract is unchanged,
    // so an SDK that ignores app_overrides still works exactly as before.
    let base_url = get_base_url();
    let body = json!({"iv": "x", "payload": "y", "supports_app_overrides": true});

    let (s, b) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 200, "POST /request should succeed: {b}");

    let v: Value = serde_json::from_str(&b).unwrap();
    let request_id = v.get("request_id").and_then(Value::as_str);
    assert!(
        request_id.is_some() && !request_id.unwrap().is_empty(),
        "request_id must still be present and non-empty: {b}"
    );
}

#[test]
fn test_response_omits_overrides_without_opt_in() {
    let base_url = get_base_url();
    let body = json!({"iv": "x", "payload": "y"});

    let (s, b) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 200, "POST /request should succeed: {b}");

    let v: Value = serde_json::from_str(&b).unwrap();
    assert!(
        v.get("request_id").and_then(Value::as_str).is_some(),
        "request_id must be present: {b}"
    );
    assert!(
        v.get("app_overrides").is_none(),
        "app_overrides must be absent when the client did not opt in: {b}"
    );
}

#[test]
fn test_create_request_without_id_still_generates_uuid() {
    let base_url = get_base_url();
    let body = json!({"iv": "no-id-iv", "payload": "no-id-payload"});

    let (s, b) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 200);
    let v: Value = serde_json::from_str(&b).unwrap();
    let id = v["request_id"].as_str().expect("request_id");
    // Auto-generated IDs are UUID v4 — 36 chars with hyphens.
    assert_eq!(id.len(), 36, "auto-generated id should be a UUID v4: {id}");
    assert_eq!(id.matches('-').count(), 4);
}

#[test]
fn test_create_request_rejects_invalid_request_id_chars() {
    let base_url = get_base_url();
    let body = json!({
        "request_id": "has spaces and / slashes",
        "iv": "x",
        "payload": "y",
    });
    let (s, _) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 400, "invalid request_id chars must 400");
}

#[test]
fn test_create_request_rejects_too_long_request_id() {
    let base_url = get_base_url();
    let body = json!({
        "request_id": "a".repeat(257),
        "iv": "x",
        "payload": "y",
    });
    let (s, _) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 400, "request_id over 256 chars must 400");
}

#[test]
fn test_create_request_rejects_too_short_request_id() {
    let base_url = get_base_url();
    let body = json!({
        "request_id": "shortid",
        "iv": "x",
        "payload": "y",
    });
    let (s, _) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 400, "request_id under 16 chars must 400");
}

#[test]
fn test_create_request_rejects_colon_in_request_id() {
    // Colon is excluded from the charset — `status:foo` would round-trip into
    // the Redis key `req:status:foo`, colliding with the bridge's own status
    // namespace. Excluding `:` prevents the overlap by construction.
    let base_url = get_base_url();
    let body = json!({
        "request_id": "status:colliding-id-here",
        "iv": "x",
        "payload": "y",
    });
    let (s, _) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 400, "request_id containing `:` must 400");
}

#[test]
fn test_get_request_malformed_path_returns_400() {
    let base_url = get_base_url();
    // 3-char path param fails the min-length check.
    let (s, _) = http_get(&format!("{base_url}/request/abc"));
    assert_eq!(s, 400, "GET /request/<too-short> must 400, not 404");
}

// ---------------------------------------------------------------------------
// request_id case-insensitivity: all IDs are normalized to lowercase so
// callers using mixed-case or uppercase IDs always hit the same Redis key.
// ---------------------------------------------------------------------------

/// Supplying an uppercase `request_id` on POST /request — the response echoes
/// it back as lowercase and the key is stored lowercase.
#[test]
fn test_create_request_uppercase_id_is_normalized() {
    let base_url = get_base_url();
    let id_lower = fresh_id();
    let id_upper = id_lower.to_uppercase();

    let body = json!({"request_id": id_upper, "iv": "iv-upper", "payload": "payload-upper"});
    let (s, b) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 200, "POST with uppercase request_id should succeed: {b}");

    let v: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(
        v["request_id"], id_lower,
        "response must echo the normalized (lowercase) id"
    );

    // Retrieve using the lowercase id — must find the stored payload.
    let (gs, gb) = http_get(&format!("{base_url}/request/{id_lower}"));
    assert_eq!(gs, 200, "GET with lowercase id must find the payload");
    let gv: Value = serde_json::from_str(&gb).unwrap();
    assert_eq!(gv["iv"], "iv-upper");
}

/// GET /request/:id with an uppercase path param finds the key stored by its
/// lowercase equivalent (normalization happens at the path layer too).
#[test]
fn test_get_request_uppercase_path_param_is_normalized() {
    let base_url = get_base_url();
    let id_lower = fresh_id();

    // Store using lowercase id.
    let body = json!({"request_id": id_lower, "iv": "iv-path", "payload": "payload-path"});
    let (s, _) = http_post(&format!("{base_url}/request"), &body);
    assert_eq!(s, 200);

    // Retrieve using uppercase path param — must hit the same key.
    let id_upper = id_lower.to_uppercase();
    let (gs, gb) = http_get(&format!("{base_url}/request/{id_upper}"));
    assert_eq!(
        gs, 200,
        "GET with uppercase path param must find the payload"
    );
    let gv: Value = serde_json::from_str(&gb).unwrap();
    assert_eq!(gv["iv"], "iv-path");
    assert_eq!(gv["payload"], "payload-path");
}

/// Full round-trip: POST with uppercase id, PUT response via uppercase path,
/// GET response via lowercase path — all resolve to the same key.
#[test]
fn test_case_insensitive_full_round_trip() {
    let base_url = get_base_url();
    let id_lower = fresh_id();
    let id_upper = id_lower.to_uppercase();

    // Create request with uppercase id.
    let req_body = json!({"request_id": id_upper, "iv": "rt-iv", "payload": "rt-payload"});
    let (s, _) = http_post(&format!("{base_url}/request"), &req_body);
    assert_eq!(s, 200);

    // Consume the request (GET) with mixed path — verifies normalization at GET.
    let (gs, _) = http_get(&format!("{base_url}/request/{id_upper}"));
    assert_eq!(gs, 200);

    // PUT response using uppercase path param.
    let res_body = json!({"iv": "rt-resp-iv", "payload": "rt-resp-payload"});
    let (ps, _) = http_put(&format!("{base_url}/response/{id_upper}"), &res_body);
    assert_eq!(ps, 201, "PUT /response with uppercase id must succeed");

    // GET response using lowercase path param.
    let (rgs, rgb) = http_get(&format!("{base_url}/response/{id_lower}"));
    assert_eq!(
        rgs, 200,
        "GET /response with lowercase id must find the response"
    );
    let rv: Value = serde_json::from_str(&rgb).unwrap();
    assert_eq!(rv["status"], "completed");
    assert_eq!(rv["response"]["iv"], "rt-resp-iv");
}

/// Test OpenAPI documentation endpoint exists
#[test]
fn test_openapi_endpoint() {
    let base_url = get_base_url();
    let url = format!("{}/openapi.json", base_url);
    let (status_code, body) = http_get(&url);

    assert_eq!(status_code, 200);

    let json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    assert!(json.get("openapi").is_some());
}

// ---------------------------------------------------------------------------
// WDP85 tracing metadata. This data lives in a separate, expiring Redis record.
// GET /request exposes the flow ID only when the caller explicitly opts in.
// ---------------------------------------------------------------------------

#[test]
fn test_flow_metadata_lifecycle_timestamps_ttls_and_cleanup() {
    let base_url = get_base_url();
    let request_id = fresh_id();
    let request = json!({
        "request_id": request_id,
        "iv": "lifecycle-request-iv",
        "payload": "lifecycle-request-payload",
    });

    let (create_status, create_body) = http_post(&format!("{base_url}/request"), &request);
    assert_eq!(create_status, 200, "request creation failed: {create_body}");
    let create_json: Value = serde_json::from_str(&create_body).unwrap();
    assert_eq!(create_json["request_id"], request_id);
    assert!(
        create_json.get("idkit_flow_id").is_none(),
        "flow identifiers must not be exposed by POST /request"
    );

    let created_metadata = flow_metadata(&request_id).expect("flow metadata after request create");
    let flow_id = created_metadata["idkit_flow_id"]
        .as_str()
        .expect("idkit_flow_id");
    Uuid::parse_str(flow_id).expect("idkit_flow_id is a UUID");
    let request_persisted_at_ms = created_metadata["request_persisted_at_ms"]
        .as_u64()
        .expect("request_persisted_at_ms");
    assert!(request_persisted_at_ms > 0);
    assert!(created_metadata.get("response_persisted_at_ms").is_none());
    let created_ttl = redis_ttl(&flow_key(&request_id));
    assert!((1..=900).contains(&created_ttl));

    let (request_status, request_body) = http_get(&format!("{base_url}/request/{request_id}"));
    assert_eq!(
        request_status, 200,
        "request consumption failed: {request_body}"
    );
    let request_json: Value = serde_json::from_str(&request_body).unwrap();
    assert_eq!(
        request_json,
        json!({
            "iv": "lifecycle-request-iv",
            "payload": "lifecycle-request-payload",
        }),
        "GET /request response schema must stay unchanged"
    );
    assert!(
        flow_metadata(&request_id).is_some(),
        "request consumption refreshes metadata for the response leg"
    );

    let response = json!({
        "iv": "lifecycle-response-iv",
        "payload": "lifecycle-response-payload",
    });
    let (put_status, put_body) = http_put(&format!("{base_url}/response/{request_id}"), &response);
    assert_eq!(put_status, 201, "response creation failed: {put_body}");

    let response_metadata =
        flow_metadata(&request_id).expect("flow metadata after response create");
    assert_eq!(response_metadata["idkit_flow_id"], flow_id);
    assert_eq!(
        response_metadata["request_persisted_at_ms"],
        request_persisted_at_ms
    );
    let response_persisted_at_ms = response_metadata["response_persisted_at_ms"]
        .as_u64()
        .expect("response_persisted_at_ms");
    assert!(response_persisted_at_ms >= request_persisted_at_ms);
    let response_ttl = redis_ttl(&flow_key(&request_id));
    assert!((1..=900).contains(&response_ttl));

    let (get_status, get_body) = http_get(&format!("{base_url}/response/{request_id}"));
    assert_eq!(get_status, 200, "response consumption failed: {get_body}");
    let get_json: Value = serde_json::from_str(&get_body).unwrap();
    assert_eq!(get_json["status"], "completed");
    assert_eq!(get_json["response"], response);
    assert!(
        get_json.get("idkit_flow_id").is_none(),
        "GET /response must not expose flow identifiers"
    );
    assert!(
        flow_metadata(&request_id).is_none(),
        "flow metadata is deleted after response consumption"
    );
    assert_eq!(redis_ttl(&format!("req:status:{request_id}")), -2);
}

#[test]
fn test_flow_ids_are_unique_and_never_replace_request_ids() {
    let base_url = get_base_url();
    let first_id = fresh_id();
    let second_id = fresh_id();

    for request_id in [&first_id, &second_id] {
        let body = json!({
            "request_id": request_id,
            "iv": "unique-iv",
            "payload": "unique-payload",
        });
        let (status, response) = http_post(&format!("{base_url}/request"), &body);
        assert_eq!(status, 200);
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["request_id"], request_id.as_str());
        assert!(response.get("idkit_flow_id").is_none());
    }

    let first_flow = flow_metadata(&first_id).unwrap();
    let second_flow = flow_metadata(&second_id).unwrap();
    assert_ne!(first_flow["idkit_flow_id"], second_flow["idkit_flow_id"]);
    assert_ne!(first_flow["idkit_flow_id"], first_id);
    assert_ne!(second_flow["idkit_flow_id"], second_id);
}

#[test]
fn test_get_request_returns_idkit_flow_id_only_when_requested() {
    let base_url = get_base_url();
    let request_id = fresh_id();
    let request = json!({
        "request_id": request_id,
        "iv": "flow-id-header-iv",
        "payload": "flow-id-header-payload",
    });

    let (create_status, create_body) = http_post(&format!("{base_url}/request"), &request);
    assert_eq!(create_status, 200, "request creation failed: {create_body}");

    let flow = flow_metadata(&request_id).expect("flow metadata after request create");
    let expected_flow_id = flow["idkit_flow_id"].as_str().expect("idkit_flow_id");

    let (get_status, get_body) = http_get_with_headers(
        &format!("{base_url}/request/{request_id}"),
        &["Accept-IDKit-Flow-ID: true"],
    );
    assert_eq!(get_status, 200, "request consumption failed: {get_body}");

    let response: Value = serde_json::from_str(&get_body).unwrap();
    assert_eq!(response["iv"], "flow-id-header-iv");
    assert_eq!(response["payload"], "flow-id-header-payload");
    assert_eq!(response["idkit_flow_id"], expected_flow_id);
}

#[test]
fn test_concurrent_consumers_keep_request_and_response_one_time() {
    const CONSUMERS: usize = 8;

    let base_url = get_base_url();
    let request_id = fresh_id();
    let request = json!({
        "request_id": request_id,
        "iv": "concurrent-request-iv",
        "payload": "concurrent-request-payload",
    });
    let (status, body) = http_post(&format!("{base_url}/request"), &request);
    assert_eq!(status, 200, "request creation failed: {body}");

    let request_url = format!("{base_url}/request/{request_id}");
    let barrier = Arc::new(Barrier::new(CONSUMERS));
    let consumers: Vec<_> = (0..CONSUMERS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let request_url = request_url.clone();
            thread::spawn(move || {
                barrier.wait();
                http_get(&request_url).0
            })
        })
        .collect();
    let statuses: Vec<_> = consumers
        .into_iter()
        .map(|consumer| consumer.join().unwrap())
        .collect();
    assert_eq!(statuses.iter().filter(|&&status| status == 200).count(), 1);
    assert_eq!(
        statuses.iter().filter(|&&status| status == 404).count(),
        CONSUMERS - 1
    );

    let response = json!({
        "iv": "concurrent-response-iv",
        "payload": "concurrent-response-payload",
    });
    let (status, body) = http_put(&format!("{base_url}/response/{request_id}"), &response);
    assert_eq!(status, 201, "response creation failed: {body}");

    let response_url = format!("{base_url}/response/{request_id}");
    let barrier = Arc::new(Barrier::new(CONSUMERS));
    let consumers: Vec<_> = (0..CONSUMERS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let response_url = response_url.clone();
            thread::spawn(move || {
                barrier.wait();
                http_get(&response_url).0
            })
        })
        .collect();
    let statuses: Vec<_> = consumers
        .into_iter()
        .map(|consumer| consumer.join().unwrap())
        .collect();
    assert_eq!(statuses.iter().filter(|&&status| status == 200).count(), 1);
    assert_eq!(
        statuses.iter().filter(|&&status| status == 404).count(),
        CONSUMERS - 1
    );
}

#[test]
fn test_legacy_missing_flow_metadata_does_not_break_round_trip() {
    let base_url = get_base_url();
    let request_id = fresh_id();
    let request = json!({
        "request_id": request_id,
        "iv": "legacy-request-iv",
        "payload": "legacy-request-payload",
    });
    let (status, body) = http_post(&format!("{base_url}/request"), &request);
    assert_eq!(status, 200, "request creation failed: {body}");

    let deleted: usize = redis_connection()
        .del(flow_key(&request_id))
        .expect("delete flow metadata");
    assert_eq!(deleted, 1);

    let (status, body) = http_get(&format!("{base_url}/request/{request_id}"));
    assert_eq!(status, 200, "legacy request consumption failed: {body}");

    let response = json!({
        "iv": "legacy-response-iv",
        "payload": "legacy-response-payload",
    });
    let (status, body) = http_put(&format!("{base_url}/response/{request_id}"), &response);
    assert_eq!(status, 201, "legacy response creation failed: {body}");

    let (status, body) = http_get(&format!("{base_url}/response/{request_id}"));
    assert_eq!(status, 200, "legacy response consumption failed: {body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["response"], response);
}

#[test]
fn test_malformed_flow_metadata_is_best_effort() {
    let base_url = get_base_url();
    let request_id = fresh_id();
    let request = json!({
        "request_id": request_id,
        "iv": "malformed-request-iv",
        "payload": "malformed-request-payload",
    });
    let (status, body) = http_post(&format!("{base_url}/request"), &request);
    assert_eq!(status, 200, "request creation failed: {body}");

    redis_connection()
        .set_ex::<_, _, ()>(flow_key(&request_id), b"not-json", 900)
        .expect("inject malformed flow metadata");

    let (status, body) = http_get(&format!("{base_url}/request/{request_id}"));
    assert_eq!(
        status, 200,
        "metadata parse failure must not break request consumption: {body}"
    );

    let response = json!({
        "iv": "malformed-response-iv",
        "payload": "malformed-response-payload",
    });
    let (status, body) = http_put(&format!("{base_url}/response/{request_id}"), &response);
    assert_eq!(
        status, 201,
        "metadata parse failure must not break response creation: {body}"
    );

    let (status, body) = http_get(&format!("{base_url}/response/{request_id}"));
    assert_eq!(
        status, 200,
        "metadata parse failure must not break response consumption: {body}"
    );
    assert!(flow_metadata(&request_id).is_none());
}

#[test]
fn test_standalone_response_does_not_create_idkit_flow_metadata() {
    let base_url = get_base_url();
    let response = json!({
        "iv": "standalone-untraced-iv",
        "payload": "standalone-untraced-payload",
    });
    let (status, body) = http_post(&format!("{base_url}/response"), &response);
    assert_eq!(status, 201, "standalone response creation failed: {body}");
    let body: Value = serde_json::from_str(&body).unwrap();
    let request_id = body["request_id"].as_str().unwrap();
    assert!(
        flow_metadata(request_id).is_none(),
        "standalone response flows stay outside WDP85 tracing"
    );
}
