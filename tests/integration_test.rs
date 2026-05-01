use base64::{engine::general_purpose::STANDARD, Engine};
use curl::easy::{Easy, List};
use serde_json::{json, Value};
use std::env;
use std::io::Read;
use std::time::Duration;
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
// Invite-code flow (WDP-73 / APP-9425) — `POST /request` (code variant) and
// `POST /code/redeem`.
// ---------------------------------------------------------------------------

/// Fresh 32-byte base64-encoded value, suitable as an `index` in tests.
/// Tests use unique indexes per call to avoid Redis-state collisions across
/// reruns and parallel tests.
fn fresh_index() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    STANDARD.encode(&bytes)
}

/// Random base64 payload of the given byte length — used as fake iv/ct.
fn fresh_b64(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    for chunk in bytes.chunks_mut(16) {
        let id = Uuid::new_v4();
        let take = chunk.len().min(16);
        chunk[..take].copy_from_slice(&id.as_bytes()[..take]);
    }
    STANDARD.encode(&bytes)
}

#[test]
fn test_code_request_happy_path_round_trip() {
    let base_url = get_base_url();
    let index = fresh_index();
    let iv = fresh_b64(12);
    let ciphertext = fresh_b64(64);

    let body = json!({
        "request_code_enabled": true,
        "index": index,
        "iv": iv,
        "payload": ciphertext,
    });
    let (status, body_str) = http_post(&format!("{}/request", base_url), &body);
    assert_eq!(status, 200, "code POST /request should succeed: {body_str}");

    let created: Value = serde_json::from_str(&body_str).expect("create body");
    let request_id = created["request_id"]
        .as_str()
        .expect("request_id")
        .to_string();
    let session_nonce = created["session_nonce"].as_str().expect("session_nonce");
    assert!(
        !session_nonce.is_empty() && session_nonce.len() >= 32,
        "session_nonce should be a non-empty token"
    );
    assert!(
        created["code_expires_at"].as_u64().is_some(),
        "code_expires_at should be a unix timestamp"
    );

    let redeem_body = json!({"index": index});
    let (rstatus, rbody) = http_post(&format!("{}/code/redeem", base_url), &redeem_body);
    assert_eq!(rstatus, 200, "redeem should succeed: {rbody}");

    let redeemed: Value = serde_json::from_str(&rbody).expect("redeem body");
    assert_eq!(redeemed["request_id"], request_id);
    assert_eq!(redeemed["iv"], iv);
    assert_eq!(redeemed["payload"], ciphertext);
    assert!(
        redeemed.get("delivery_token").is_none(),
        "delivery_token must not be returned"
    );
}

#[test]
fn test_code_request_duplicate_index_returns_409() {
    let base_url = get_base_url();
    let index = fresh_index();

    let body = json!({
        "request_code_enabled": true,
        "index": index,
        "iv": fresh_b64(12),
        "payload": fresh_b64(64),
    });
    let (s1, _) = http_post(&format!("{}/request", base_url), &body);
    assert_eq!(s1, 200);

    // Reuse the same index — should collide.
    let body2 = json!({
        "request_code_enabled": true,
        "index": index,
        "iv": fresh_b64(12),
        "payload": fresh_b64(64),
    });
    let (s2, _) = http_post(&format!("{}/request", base_url), &body2);
    assert_eq!(s2, 409, "duplicate live index must return 409");
}

#[test]
fn test_code_redeem_double_redeem_returns_404() {
    let base_url = get_base_url();
    let index = fresh_index();

    let body = json!({
        "request_code_enabled": true,
        "index": index,
        "iv": fresh_b64(12),
        "payload": fresh_b64(64),
    });
    let (s, _) = http_post(&format!("{}/request", base_url), &body);
    assert_eq!(s, 200);

    let redeem = json!({"index": index});
    let (s1, _) = http_post(&format!("{}/code/redeem", base_url), &redeem);
    assert_eq!(s1, 200);

    let (s2, _) = http_post(&format!("{}/code/redeem", base_url), &redeem);
    assert_eq!(s2, 404, "second redeem of the same index must 404");
}

#[test]
fn test_code_redeem_unknown_index_returns_404() {
    let base_url = get_base_url();
    // Well-formed but never inserted.
    let redeem = json!({"index": fresh_index()});
    let (s, _) = http_post(&format!("{}/code/redeem", base_url), &redeem);
    assert_eq!(s, 404);
}

#[test]
fn test_code_redeem_malformed_index_returns_404() {
    let base_url = get_base_url();
    // Not base64 — same 404 shape as missing/redeemed/expired so we don't
    // give callers an oracle on the shape of the code space.
    let redeem = json!({"index": "!!!not-base64!!!"});
    let (s, _) = http_post(&format!("{}/code/redeem", base_url), &redeem);
    assert_eq!(s, 404);
}

#[test]
fn test_code_request_rejects_invalid_index_with_400() {
    let base_url = get_base_url();
    let body = json!({
        "request_code_enabled": true,
        "index": "!!!not-base64!!!",
        "iv": fresh_b64(12),
        "payload": fresh_b64(64),
    });
    let (s, _) = http_post(&format!("{}/request", base_url), &body);
    assert_eq!(s, 400);
}

#[test]
fn test_code_request_missing_index_with_flag_returns_400() {
    let base_url = get_base_url();
    let body = json!({
        "request_code_enabled": true,
        "iv": fresh_b64(12),
        "payload": fresh_b64(64),
    });
    let (s, _) = http_post(&format!("{}/request", base_url), &body);
    assert_eq!(s, 400);
}

#[test]
fn test_legacy_request_unchanged_when_flag_false() {
    let base_url = get_base_url();
    // request_code_enabled: false should be byte-identical to the unflagged call.
    let body = json!({
        "request_code_enabled": false,
        "iv": "legacy_iv_via_false_flag",
        "payload": "legacy_payload_via_false_flag",
    });
    let (s, body_str) = http_post(&format!("{}/request", base_url), &body);
    assert_eq!(s, 200);

    let created: Value = serde_json::from_str(&body_str).expect("create body");
    let request_id = created["request_id"].as_str().expect("request_id");
    assert!(
        created.get("session_nonce").is_none(),
        "legacy must not return session_nonce"
    );
    assert!(
        created.get("code_expires_at").is_none(),
        "legacy must not return code_expires_at"
    );

    // And the legacy GET /request/:id flow still works.
    let (gs, gbody) = http_get(&format!("{}/request/{}", base_url, request_id));
    assert_eq!(gs, 200);
    let got: Value = serde_json::from_str(&gbody).expect("get body");
    assert_eq!(got["iv"], "legacy_iv_via_false_flag");
    assert_eq!(got["payload"], "legacy_payload_via_false_flag");
}

#[test]
fn test_code_redeem_concurrent_exactly_one_winner() {
    let base_url = get_base_url();
    let index = fresh_index();

    let body = json!({
        "request_code_enabled": true,
        "index": index,
        "iv": fresh_b64(12),
        "payload": fresh_b64(64),
    });
    let (s, _) = http_post(&format!("{}/request", base_url), &body);
    assert_eq!(s, 200);

    let url = format!("{}/code/redeem", base_url);
    let payload = json!({"index": index});

    let handles: Vec<_> = (0..50)
        .map(|_| {
            let url = url.clone();
            let payload = payload.clone();
            std::thread::spawn(move || http_post(&url, &payload))
        })
        .collect();

    let results: Vec<(u32, String)> = handles
        .into_iter()
        .map(|h| h.join().expect("thread panic"))
        .collect();

    let winners: Vec<&(u32, String)> = results.iter().filter(|(s, _)| *s == 200).collect();
    let losers: Vec<&(u32, String)> = results.iter().filter(|(s, _)| *s == 404).collect();
    assert_eq!(
        winners.len(),
        1,
        "exactly one redeem must win; got {} winners",
        winners.len()
    );
    assert_eq!(
        losers.len(),
        49,
        "all other redeems must 404; got {} losers",
        losers.len()
    );
}

/// TTL expiry test. Requires the server **and** this test runner to be started
/// with `CODE_TTL_SECONDS` set to a small value (recommend 2). Fails fast with
/// a clear message if the env var is missing — running against a 10-minute
/// production TTL would block CI for far too long.
#[test]
fn test_code_expires_after_ttl() {
    let base_url = get_base_url();
    let ttl_secs: u64 = env::var("CODE_TTL_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .expect(
            "set CODE_TTL_SECONDS=2 (or similar small value) on BOTH the server \
             process and the test runner before running this test",
        );
    assert!(
        ttl_secs <= 10,
        "CODE_TTL_SECONDS={ttl_secs} is too long for the expiry test"
    );

    let index = fresh_index();
    let body = json!({
        "request_code_enabled": true,
        "index": index,
        "iv": fresh_b64(12),
        "payload": fresh_b64(64),
    });
    let (s, _) = http_post(&format!("{}/request", base_url), &body);
    assert_eq!(s, 200);

    // Sleep long enough for the code to expire, with a 1s margin.
    std::thread::sleep(Duration::from_secs(ttl_secs + 1));

    let (rs, _) = http_post(
        &format!("{}/code/redeem", base_url),
        &json!({"index": index}),
    );
    assert_eq!(rs, 404, "expired code must redeem to 404");
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
