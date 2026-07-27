use serde_json::{json, Value};
use uuid::Uuid;

mod common;

/// Test the root endpoint returns service information
#[tokio::test]
async fn test_root_endpoint() {
    let app = common::test_app().await;
    let (status_code, body) = common::get(&app, "/").await;

    assert_eq!(status_code, 200);

    let json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    assert!(json.get("repo_url").is_some());
    assert!(json.get("version").is_some());
}

/// Test POST /request creates a new request and returns a request_id
#[tokio::test]
async fn test_create_request() {
    let app = common::test_app().await;
    let payload = json!({
        "iv": "test_iv",
        "payload": "test_payload"
    });

    let (status_code, body) = common::post(&app, "/request", &payload).await;

    assert_eq!(status_code, 200);

    let json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = json.get("request_id").expect("Missing request_id");
    assert!(request_id.is_string());
    assert!(!request_id.as_str().unwrap().is_empty());
}

/// Test GET /request/:id retrieves the request (one-time use)
#[tokio::test]
async fn test_get_request_one_time_use() {
    let app = common::test_app().await;

    // Create a request
    let payload = json!({
        "iv": "get_test_iv",
        "payload": "get_test_payload"
    });

    let (status_code, body) = common::post(&app, "/request", &payload).await;
    assert_eq!(status_code, 200);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // First GET should succeed
    let get_url = format!("/request/{request_id}");
    let (first_status, first_body) = common::get(&app, &get_url).await;

    assert_eq!(first_status, 200);

    let first_json: Value = serde_json::from_str(&first_body).expect("Failed to parse JSON");
    assert_eq!(first_json["iv"], "get_test_iv");
    assert_eq!(first_json["payload"], "get_test_payload");

    // Second GET should fail (one-time use)
    let (second_status, _) = common::get(&app, &get_url).await;
    assert_eq!(second_status, 404);
}

/// Test PUT /response/:id stores a response for a request
#[tokio::test]
async fn test_put_response_for_request() {
    let app = common::test_app().await;

    // Create a request
    let request_payload = json!({
        "iv": "request_iv",
        "payload": "request_payload"
    });

    let (status_code, body) = common::post(&app, "/request", &request_payload).await;
    assert_eq!(status_code, 200);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // Put a response
    let response_payload = json!({
        "iv": "response_iv",
        "payload": "response_payload"
    });

    let put_url = format!("/response/{request_id}");
    let (put_status, _) = common::put(&app, &put_url, &response_payload).await;

    assert_eq!(put_status, 201);
}

/// Test GET /response/:id retrieves the response
#[tokio::test]
async fn test_get_response() {
    let app = common::test_app().await;

    // Create a request
    let request_payload = json!({
        "iv": "req_iv",
        "payload": "req_payload"
    });

    let (status_code, body) = common::post(&app, "/request", &request_payload).await;
    assert_eq!(status_code, 200);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // Put a response
    let response_payload = json!({
        "iv": "resp_iv",
        "payload": "resp_payload"
    });

    let put_url = format!("/response/{request_id}");
    let (put_status, _) = common::put(&app, &put_url, &response_payload).await;
    assert_eq!(put_status, 201);

    // Get the response
    let get_url = format!("/response/{request_id}");
    let (get_status, get_body) = common::get(&app, &get_url).await;

    assert_eq!(get_status, 200);

    let json: Value = serde_json::from_str(&get_body).expect("Failed to parse JSON");
    assert_eq!(json["status"], "completed");
    assert_eq!(json["response"]["iv"], "resp_iv");
    assert_eq!(json["response"]["payload"], "resp_payload");
}

/// Test GET /response/:id returns pending status when response not yet submitted
#[tokio::test]
async fn test_response_pending_status() {
    let app = common::test_app().await;

    // Create a request
    let request_payload = json!({
        "iv": "pending_iv",
        "payload": "pending_payload"
    });

    let (status_code, body) = common::post(&app, "/request", &request_payload).await;
    assert_eq!(status_code, 200);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // Get response before it's been PUT (should show pending)
    let get_url = format!("/response/{request_id}");
    let (get_status, get_body) = common::get(&app, &get_url).await;

    assert_eq!(get_status, 200);

    let json: Value = serde_json::from_str(&get_body).expect("Failed to parse JSON");
    let status = json["status"].as_str().unwrap();
    assert!(status == "initialized" || status == "retrieved" || status == "pending");
}

/// Test POST /response creates a standalone response
#[tokio::test]
async fn test_create_standalone_response() {
    let app = common::test_app().await;

    let payload = json!({
        "iv": "standalone_iv",
        "payload": "standalone_payload"
    });

    let (status_code, body) = common::post(&app, "/response", &payload).await;

    assert_eq!(status_code, 201);

    let json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = json.get("request_id").expect("Missing request_id");
    assert!(request_id.is_string());
}

/// Test standalone response flow: POST /response then GET /response/:id
#[tokio::test]
async fn test_standalone_response_flow() {
    let app = common::test_app().await;

    // Create standalone response
    let payload = json!({
        "iv": "standalone_flow_iv",
        "payload": "standalone_flow_payload"
    });

    let (status_code, body) = common::post(&app, "/response", &payload).await;
    assert_eq!(status_code, 201);

    let create_json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    let request_id = create_json["request_id"].as_str().unwrap();

    // Retrieve the response
    let get_url = format!("/response/{request_id}");
    let (get_status, get_body) = common::get(&app, &get_url).await;

    assert_eq!(get_status, 200);

    let json: Value = serde_json::from_str(&get_body).expect("Failed to parse JSON");
    assert_eq!(json["status"], "completed");
    assert_eq!(json["response"]["iv"], "standalone_flow_iv");
    assert_eq!(json["response"]["payload"], "standalone_flow_payload");
}

/// Test invalid request ID returns 404
#[tokio::test]
async fn test_invalid_request_id() {
    let app = common::test_app().await;
    let (status_code, _) = common::get(&app, "/request/00000000-0000-0000-0000-000000000000").await;

    assert_eq!(status_code, 404);
}

/// Test invalid response ID returns appropriate error
#[tokio::test]
async fn test_invalid_response_id() {
    let app = common::test_app().await;
    let (status_code, _) =
        common::get(&app, "/response/00000000-0000-0000-0000-000000000000").await;

    // Should return 404 or an error status
    assert!(status_code == 404 || (400..500).contains(&status_code));
}

/// Test that request requires both iv and payload fields
#[tokio::test]
async fn test_create_request_validation() {
    let app = common::test_app().await;

    // Missing payload field
    let invalid_payload = json!({
        "iv": "test_iv"
    });

    let (status_code, _) = common::post(&app, "/request", &invalid_payload).await;

    assert!((400..500).contains(&status_code));
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

#[tokio::test]
async fn test_create_request_with_client_supplied_id() {
    let app = common::test_app().await;
    let id = fresh_id();
    let body = json!({
        "request_id": id,
        "iv": "iv-for-custom-id",
        "payload": "payload-for-custom-id",
    });
    let (s, b) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 200, "POST /request with custom id should succeed: {b}");
    let v: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(v["request_id"], id, "response echoes the supplied id");

    let (gs, gb) = common::get(&app, &format!("/request/{id}")).await;
    assert_eq!(gs, 200, "GET /request/<custom> should retrieve the payload");
    let gv: Value = serde_json::from_str(&gb).unwrap();
    assert_eq!(gv["iv"], "iv-for-custom-id");
    assert_eq!(gv["payload"], "payload-for-custom-id");

    // GETDEL semantics — second GET 404s.
    let (gs2, _) = common::get(&app, &format!("/request/{id}")).await;
    assert_eq!(gs2, 404);
}

#[tokio::test]
async fn test_create_request_with_duplicate_id_returns_409() {
    let app = common::test_app().await;
    let id = fresh_id();
    let body = json!({"request_id": id, "iv": "x", "payload": "y"});

    let (s1, _) = common::post(&app, "/request", &body).await;
    assert_eq!(s1, 200);

    let (s2, _) = common::post(&app, "/request", &body).await;
    assert_eq!(s2, 409, "second POST with same request_id must 409");
}

// ---------------------------------------------------------------------------
// Server-driven URL overrides. The bridge takes no `app_id` input — it returns
// the whole `app_overrides` map blindly and the SDK picks its own entry. The
// harness injects the fixture directly into the router (see `common`), so these
// tests need no `APP_URL_OVERRIDES` env var.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_response_includes_full_override_map() {
    let app = common::test_app().await;
    // Opt in — the bridge only returns overrides to clients that advertise support.
    let body = json!({"iv": "x", "payload": "y", "supports_app_overrides": true});

    let (s, b) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 200, "POST /request should succeed: {b}");

    let v: Value = serde_json::from_str(&b).unwrap();
    let entry = v
        .get("app_overrides")
        .and_then(|m| m.get(common::FIXTURE_APP_ID))
        .unwrap_or_else(|| panic!("app_overrides map must contain the fixture entry: {b}"));

    assert_eq!(
        entry.get("app_clip_bundle_id").and_then(Value::as_str),
        Some(common::FIXTURE_APP_CLIP_BUNDLE_ID),
        "fixture entry must carry the configured app_clip_bundle_id: {b}"
    );
    assert_eq!(
        entry.get("verify_url").and_then(Value::as_str),
        Some(common::FIXTURE_VERIFY_URL),
        "fixture entry must carry the configured verify_url: {b}"
    );
}

#[tokio::test]
async fn test_request_id_still_present_alongside_overrides() {
    // The override map is additive — the core request_id contract is unchanged,
    // so an SDK that ignores app_overrides still works exactly as before.
    let app = common::test_app().await;
    let body = json!({"iv": "x", "payload": "y", "supports_app_overrides": true});

    let (s, b) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 200, "POST /request should succeed: {b}");

    let v: Value = serde_json::from_str(&b).unwrap();
    let request_id = v.get("request_id").and_then(Value::as_str);
    assert!(
        request_id.is_some() && !request_id.unwrap().is_empty(),
        "request_id must still be present and non-empty: {b}"
    );
}

#[tokio::test]
async fn test_response_omits_overrides_without_opt_in() {
    let app = common::test_app().await;
    let body = json!({"iv": "x", "payload": "y"});

    let (s, b) = common::post(&app, "/request", &body).await;
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

#[tokio::test]
async fn test_create_request_without_id_still_generates_uuid() {
    let app = common::test_app().await;
    let body = json!({"iv": "no-id-iv", "payload": "no-id-payload"});

    let (s, b) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 200);
    let v: Value = serde_json::from_str(&b).unwrap();
    let id = v["request_id"].as_str().expect("request_id");
    // Auto-generated IDs are UUID v4 — 36 chars with hyphens.
    assert_eq!(id.len(), 36, "auto-generated id should be a UUID v4: {id}");
    assert_eq!(id.matches('-').count(), 4);
}

#[tokio::test]
async fn test_create_request_rejects_invalid_request_id_chars() {
    let app = common::test_app().await;
    let body = json!({
        "request_id": "has spaces and / slashes",
        "iv": "x",
        "payload": "y",
    });
    let (s, _) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 400, "invalid request_id chars must 400");
}

#[tokio::test]
async fn test_create_request_rejects_too_long_request_id() {
    let app = common::test_app().await;
    let body = json!({
        "request_id": "a".repeat(257),
        "iv": "x",
        "payload": "y",
    });
    let (s, _) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 400, "request_id over 256 chars must 400");
}

#[tokio::test]
async fn test_create_request_rejects_too_short_request_id() {
    let app = common::test_app().await;
    let body = json!({
        "request_id": "shortid",
        "iv": "x",
        "payload": "y",
    });
    let (s, _) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 400, "request_id under 16 chars must 400");
}

#[tokio::test]
async fn test_create_request_rejects_colon_in_request_id() {
    // Colon is excluded from the charset — `status:foo` would round-trip into
    // the Redis key `req:status:foo`, colliding with the bridge's own status
    // namespace. Excluding `:` prevents the overlap by construction.
    let app = common::test_app().await;
    let body = json!({
        "request_id": "status:colliding-id-here",
        "iv": "x",
        "payload": "y",
    });
    let (s, _) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 400, "request_id containing `:` must 400");
}

#[tokio::test]
async fn test_get_request_malformed_path_returns_400() {
    let app = common::test_app().await;
    // 3-char path param fails the min-length check.
    let (s, _) = common::get(&app, "/request/abc").await;
    assert_eq!(s, 400, "GET /request/<too-short> must 400, not 404");
}

// ---------------------------------------------------------------------------
// request_id case-insensitivity: all IDs are normalized to lowercase so
// callers using mixed-case or uppercase IDs always hit the same Redis key.
// ---------------------------------------------------------------------------

/// Supplying an uppercase `request_id` on POST /request — the response echoes
/// it back as lowercase and the key is stored lowercase.
#[tokio::test]
async fn test_create_request_uppercase_id_is_normalized() {
    let app = common::test_app().await;
    let id_lower = fresh_id();
    let id_upper = id_lower.to_uppercase();

    let body = json!({"request_id": id_upper, "iv": "iv-upper", "payload": "payload-upper"});
    let (s, b) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 200, "POST with uppercase request_id should succeed: {b}");

    let v: Value = serde_json::from_str(&b).unwrap();
    assert_eq!(
        v["request_id"], id_lower,
        "response must echo the normalized (lowercase) id"
    );

    // Retrieve using the lowercase id — must find the stored payload.
    let (gs, gb) = common::get(&app, &format!("/request/{id_lower}")).await;
    assert_eq!(gs, 200, "GET with lowercase id must find the payload");
    let gv: Value = serde_json::from_str(&gb).unwrap();
    assert_eq!(gv["iv"], "iv-upper");
}

/// GET /request/:id with an uppercase path param finds the key stored by its
/// lowercase equivalent (normalization happens at the path layer too).
#[tokio::test]
async fn test_get_request_uppercase_path_param_is_normalized() {
    let app = common::test_app().await;
    let id_lower = fresh_id();

    // Store using lowercase id.
    let body = json!({"request_id": id_lower, "iv": "iv-path", "payload": "payload-path"});
    let (s, _) = common::post(&app, "/request", &body).await;
    assert_eq!(s, 200);

    // Retrieve using uppercase path param — must hit the same key.
    let id_upper = id_lower.to_uppercase();
    let (gs, gb) = common::get(&app, &format!("/request/{id_upper}")).await;
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
#[tokio::test]
async fn test_case_insensitive_full_round_trip() {
    let app = common::test_app().await;
    let id_lower = fresh_id();
    let id_upper = id_lower.to_uppercase();

    // Create request with uppercase id.
    let req_body = json!({"request_id": id_upper, "iv": "rt-iv", "payload": "rt-payload"});
    let (s, _) = common::post(&app, "/request", &req_body).await;
    assert_eq!(s, 200);

    // Consume the request (GET) with mixed path — verifies normalization at GET.
    let (gs, _) = common::get(&app, &format!("/request/{id_upper}")).await;
    assert_eq!(gs, 200);

    // PUT response using uppercase path param.
    let res_body = json!({"iv": "rt-resp-iv", "payload": "rt-resp-payload"});
    let (ps, _) = common::put(&app, &format!("/response/{id_upper}"), &res_body).await;
    assert_eq!(ps, 201, "PUT /response with uppercase id must succeed");

    // GET response using lowercase path param.
    let (rgs, rgb) = common::get(&app, &format!("/response/{id_lower}")).await;
    assert_eq!(
        rgs, 200,
        "GET /response with lowercase id must find the response"
    );
    let rv: Value = serde_json::from_str(&rgb).unwrap();
    assert_eq!(rv["status"], "completed");
    assert_eq!(rv["response"]["iv"], "rt-resp-iv");
}

/// Test OpenAPI documentation endpoint exists
#[tokio::test]
async fn test_openapi_endpoint() {
    let app = common::test_app().await;
    let (status_code, body) = common::get(&app, "/openapi.json").await;

    assert_eq!(status_code, 200);

    let json: Value = serde_json::from_str(&body).expect("Failed to parse JSON");
    assert!(json.get("openapi").is_some());
}
