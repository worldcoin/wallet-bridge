use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    thread,
};

fn invoke(args: &[&str], input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bridge-cli"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn exchange(
    args: &[&str],
    input: &str,
    status: &str,
    body: &str,
) -> (std::process::Output, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/bridge/", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut received = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            received.extend_from_slice(&buffer[..count]);
            if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&received[..end]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map_or(0, |n| n.parse::<usize>().unwrap());
                if received.len() >= end + 4 + length {
                    break;
                }
            }
        }
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8(received).unwrap()
    });
    let mut all_args = vec!["--url", &url];
    all_args.extend_from_slice(args);
    let output = invoke(&all_args, input);
    (output, server.join().unwrap())
}

#[test]
fn creates_ciphertext_with_custom_id() {
    let (output, request) = exchange(
        &["create", "request", "--id", "ABCDEF0123456789"],
        r#"{"iv":"opaque-IV","payload":"opaque ciphertext"}"#,
        "200 OK",
        r#"{"request_id":"abcdef0123456789"}"#,
    );
    assert!(output.status.success());
    assert!(request.starts_with("POST /bridge/request HTTP/1.1"));
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"iv":"opaque-IV", "payload":"opaque ciphertext", "request_id":"abcdef0123456789"})
    );
    assert_eq!(output.stdout, b"{\"request_id\":\"abcdef0123456789\"}\n");
}

#[test]
fn routes_each_operation_and_returns_response_verbatim() {
    for (args, method, path, input) in [
        (
            vec!["create", "response"],
            "POST",
            "response",
            r#"{"iv":"i","payload":"p"}"#,
        ),
        (
            vec!["respond", "abcdef0123456789"],
            "PUT",
            "response/abcdef0123456789",
            r#"{"iv":"i","payload":"p"}"#,
        ),
        (
            vec!["get", "request", "abcdef0123456789"],
            "GET",
            "request/abcdef0123456789",
            "",
        ),
        (
            vec!["get", "response", "abcdef0123456789"],
            "GET",
            "response/abcdef0123456789",
            "",
        ),
    ] {
        let (output, request) = exchange(&args, input, "200 OK", "{}");
        assert!(output.status.success());
        assert!(request.starts_with(&format!("{method} /bridge/{path} HTTP/1.1")));
        assert_eq!(output.stdout, b"{}\n");
    }
}

#[test]
fn head_checks_and_http_failures() {
    for resource in ["request", "response"] {
        for (status, success) in [("200 OK", true), ("404 Not Found", false)] {
            let (output, request) =
                exchange(&["head", resource, "abcdef0123456789"], "", status, "");
            assert_eq!(output.status.success(), success);
            assert!(request.starts_with("HEAD "));
            assert_eq!(
                String::from_utf8(output.stdout).unwrap().trim(),
                &status[..3]
            );
        }
    }
    for status in ["409 Conflict", "302 Found", "500 Internal Server Error"] {
        let (output, _) = exchange(
            &["get", "response", "abcdef0123456789"],
            "",
            status,
            "do not echo this",
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("do not echo this"));
    }
}

#[test]
fn invalid_input_fails_before_network_io() {
    for (args, input) in [
        (vec!["get", "request", "bad/id"], ""),
        (vec!["create", "response", "--id", "abcdef0123456789"], ""),
        (vec!["create", "request"], "secret-invalid-json"),
        (vec!["create", "request"], r#"{"iv":"i","payload":123}"#),
        (
            vec!["create", "request"],
            r#"{"iv":"i","payload":"p","extra":true}"#,
        ),
    ] {
        let output = invoke(&args, input);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("secret-invalid-json"));
    }
}
