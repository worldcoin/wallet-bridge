use super::{parse_id, Resource};
use base64::{engine::general_purpose::STANDARD, Engine};
use clap::Args;
use reqwest::{Client, Method, Url};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Args)]
pub struct Input {
    /// Plaintext message to encrypt (may be visible in shell history).
    #[arg(long, conflicts_with = "input")]
    message: Option<String>,
    /// Plaintext file, or - for stdin. Reads stdin when neither option is given.
    #[arg(long)]
    input: Option<PathBuf>,
}
impl Input {
    fn read(self) -> Result<Vec<u8>> {
        if let Some(message) = self.message {
            return Ok(message.into_bytes());
        }
        if let Some(path) = self.input {
            if path.as_os_str() != "-" {
                return Ok(std::fs::read(path)?);
            }
        }
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

#[derive(Args)]
pub struct Send {
    #[command(flatten)]
    input: Input,
    /// Existing base64 AES-256 key; otherwise generate a fresh key locally.
    #[arg(long, env = "BRIDGE_KEY", hide_env_values = true)]
    key: Option<String>,
}
#[derive(Args)]
pub struct Receive {
    #[arg(value_enum)]
    resource: Resource,
    #[arg(value_parser = parse_id)]
    id: String,
    /// Base64 AES-256 key from send. Never sent to the bridge.
    #[arg(long, env = "BRIDGE_KEY", hide_env_values = true)]
    key: String,
}
#[derive(Args)]
pub struct Reply {
    #[arg(value_parser = parse_id)]
    id: String,
    #[arg(long, env = "BRIDGE_KEY", hide_env_values = true)]
    key: String,
    #[command(flatten)]
    input: Input,
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "secure random generation failed")?;
    Ok(bytes)
}
fn decode_key(encoded: &str) -> Result<[u8; 32]> {
    STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| "key must be a base64-encoded 32-byte AES-256 key".into())
}
fn cipher(key: &[u8; 32]) -> Result<aead::LessSafeKey> {
    Ok(aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, key).map_err(|_| "invalid encryption key")?,
    ))
}
fn encrypt(key: &[u8; 32], mut plaintext: Vec<u8>) -> Result<Value> {
    let iv = random::<12>()?;
    cipher(key)?
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(iv),
            aead::Aad::empty(),
            &mut plaintext,
        )
        .map_err(|_| "encryption failed")?;
    Ok(json!({"iv": STANDARD.encode(iv), "payload": STANDARD.encode(plaintext)}))
}
fn decrypt(key: &[u8; 32], envelope: &Value) -> Result<Vec<u8>> {
    let iv: [u8; 12] = STANDARD
        .decode(envelope["iv"].as_str().ok_or("missing IV")?)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or("invalid IV")?;
    let mut bytes = STANDARD
        .decode(envelope["payload"].as_str().ok_or("missing ciphertext")?)
        .map_err(|_| "invalid ciphertext encoding")?;
    let plaintext = cipher(key)?
        .open_in_place(
            aead::Nonce::assume_unique_for_key(iv),
            aead::Aad::empty(),
            &mut bytes,
        )
        .map_err(|_| {
            "decryption failed: incorrect key or modified ciphertext (message has been consumed)"
        })?;
    Ok(plaintext.to_vec())
}
async fn exchange(
    client: &Client,
    base: &Url,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Vec<u8>> {
    let mut url = base.clone();
    url.set_path(&format!("{}/{path}", base.path().trim_end_matches('/')));
    let mut request = client.request(method, url);
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body)?);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(format!("bridge returned HTTP {}", response.status()).into());
    }
    Ok(response.bytes().await?.to_vec())
}
pub async fn send(client: &Client, url: &Url, args: Send) -> Result<()> {
    let key = args.key.as_deref().map_or_else(random::<32>, decode_key)?;
    let body = encrypt(&key, args.input.read()?)?;
    let response: Value =
        serde_json::from_slice(&exchange(client, url, Method::POST, "request", Some(body)).await?)?;
    let id = parse_id(
        response["request_id"]
            .as_str()
            .ok_or("bridge did not return a request ID")?,
    )?;
    println!(
        "{}",
        json!({"request_id":id, "key":STANDARD.encode(key), "bridge_url":url.as_str()})
    );
    Ok(())
}
pub async fn reply(client: &Client, url: &Url, args: Reply) -> Result<()> {
    let key = decode_key(&args.key)?;
    let body = encrypt(&key, args.input.read()?)?;
    exchange(
        client,
        url,
        Method::PUT,
        &format!("response/{}", args.id),
        Some(body),
    )
    .await?;
    Ok(())
}
pub async fn receive(client: &Client, url: &Url, args: Receive) -> Result<()> {
    let key = decode_key(&args.key)?;
    let response: Value = serde_json::from_slice(
        &exchange(
            client,
            url,
            Method::GET,
            &format!("{}/{}", args.resource.path(), args.id),
            None,
        )
        .await?,
    )?;
    let envelope = match args.resource {
        Resource::Request => &response,
        Resource::Response => {
            if response["response"].is_null() {
                return Err(format!(
                    "response not ready (status: {}); try again before expiry",
                    response["status"]
                )
                .into());
            }
            &response["response"]
        }
    };
    io::stdout().lock().write_all(&decrypt(&key, envelope)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decrypts_nist_aes256_gcm_empty_plaintext_vector() {
        let tag = [
            0x53, 0x0f, 0x8a, 0xfb, 0xc7, 0x45, 0x36, 0xb9, 0xa9, 0x63, 0xb4, 0xf1, 0xc4, 0xcb,
            0x73, 0x8b,
        ];
        let envelope = json!({"iv":STANDARD.encode([0; 12]), "payload":STANDARD.encode(tag)});
        assert_eq!(decrypt(&[0; 32], &envelope).unwrap(), Vec::<u8>::new());
    }
    #[test]
    fn authenticated_round_trip_and_fresh_nonces() {
        let key = [7; 32];
        let message = b"hello\0\xff\n".to_vec();
        let first = encrypt(&key, message.clone()).unwrap();
        let second = encrypt(&key, message.clone()).unwrap();
        assert_ne!(first["iv"], second["iv"]);
        assert_eq!(decrypt(&key, &first).unwrap(), message);
        assert!(decrypt(&[8; 32], &first).is_err());
        let mut tampered = first;
        let mut bytes = STANDARD
            .decode(tampered["payload"].as_str().unwrap())
            .unwrap();
        bytes[0] ^= 1;
        tampered["payload"] = STANDARD.encode(bytes).into();
        assert!(decrypt(&key, &tampered).is_err());
    }
}
