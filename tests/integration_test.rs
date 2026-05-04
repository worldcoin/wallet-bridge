use curl::easy::{Easy, List};
use serde_json::{json, Value};
use std::env;
use std::io::Read;
use uuid::Uuid;

/// Helper to get the base URL for the wallet bridge service
fn get_base_url() -> String {
    env::var("WALLET_BRIDGE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string())
}

/// Helper to perform a GET request and return (status_code, body)
fn http_get(url: &str) -> (u32, String) {
    let mut easy = Easy::new();
    easy.url(url).unwrap();

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
