#![deny(clippy::all, clippy::pedantic, clippy::nursery)]

#[path = "bridge-cli/high_level.rs"]
mod high_level;

use clap::{Parser, Subcommand, ValueEnum};
use reqwest::{Client, Method, Url};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};
use world_id_bridge::utils::validate_request_id;

#[derive(Parser)]
#[command(
    version,
    about = "Send encrypted messages and receive replies through the bridge"
)]
struct Args {
    #[arg(
        long,
        env = "BRIDGE_URL",
        default_value = "https://staging-bridge.worldcoin.org",
        global = true
    )]
    url: Url,
    /// HTTP timeout in seconds. Requests are never retried.
    #[arg(long, default_value = "30", value_parser = clap::value_parser!(u64).range(1..), global = true)]
    timeout: u64,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Resource {
    Request,
    Response,
}
impl Resource {
    const fn path(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Encrypt a message/file and create a request; print connection details as JSON.
    Send(high_level::Send),
    /// Retrieve and decrypt a request or response once.
    Receive(high_level::Receive),
    /// Encrypt a message/file and reply to an existing request.
    Reply(high_level::Reply),
    /// Create a request or standalone response from ciphertext JSON.
    Create {
        resource: Resource,
        /// Read JSON from this file; omit or use - for stdin.
        #[arg(long, default_value = "-")]
        input: PathBuf,
        /// Supply a high-entropy ID (requests only).
        #[arg(long, value_parser = parse_id)]
        id: Option<String>,
    },
    /// Fetch once. This CONSUMES the stored message; response may still be pending.
    Get {
        resource: Resource,
        #[arg(value_parser = parse_id)]
        id: String,
    },
    /// Non-consuming existence check. Prints the HTTP status code.
    Head {
        resource: Resource,
        #[arg(value_parser = parse_id)]
        id: String,
    },
    /// Submit an encrypted response to an existing request.
    Respond {
        #[arg(value_parser = parse_id)]
        id: String,
        #[arg(long, default_value = "-")]
        input: PathBuf,
    },
}

fn parse_id(value: &str) -> Result<String, String> {
    validate_request_id(value).map_err(|_| {
        "ID must be 16–256 ASCII letters, digits, hyphens, underscores or dots".to_owned()
    })?;
    Ok(value.to_lowercase())
}

fn read_payload(path: &PathBuf) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    if path.as_os_str() == "-" {
        io::stdin().read_to_end(&mut bytes)?;
    } else {
        bytes = std::fs::read(path)?;
    }
    // Validate the envelope only; never interpret ciphertext or echo invalid input.
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "input must be JSON with string iv and payload fields")?;
    let valid = value.as_object().is_some_and(|obj| {
        obj.len() == 2
            && obj.get("iv").is_some_and(serde_json::Value::is_string)
            && obj.get("payload").is_some_and(serde_json::Value::is_string)
    });
    if !valid {
        return Err(
            "input must contain exactly string iv and payload fields; encrypt before submitting"
                .into(),
        );
    }
    Ok(value)
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut url = args.url;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("bridge URL must be HTTP(S), without credentials, query or fragment".into());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(args.timeout))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()?;
    let command = match args.command {
        Command::Send(send) => return high_level::send(&client, &url, send).await,
        Command::Receive(receive) => return high_level::receive(&client, &url, receive).await,
        Command::Reply(reply) => return high_level::reply(&client, &url, reply).await,
        command => command,
    };
    let (method, resource, id, body) = match command {
        Command::Send(_) | Command::Receive(_) | Command::Reply(_) => unreachable!(),
        Command::Create {
            resource,
            input,
            id,
        } => {
            if matches!(resource, Resource::Response) && id.is_some() {
                return Err("--id is supported only for create request".into());
            }
            let mut body = read_payload(&input)?;
            if let Some(id) = id {
                body["request_id"] = id.into();
            }
            (Method::POST, resource, None, Some(body))
        }
        Command::Get { resource, id } => (Method::GET, resource, Some(id), None),
        Command::Head { resource, id } => (Method::HEAD, resource, Some(id), None),
        Command::Respond { id, input } => (
            Method::PUT,
            Resource::Response,
            Some(id),
            Some(read_payload(&input)?),
        ),
    };
    let base = url.path().trim_end_matches('/').to_owned();
    url.set_path(&format!(
        "{base}/{}{}",
        resource.path(),
        id.map_or_else(String::new, |id| format!("/{id}"))
    ));
    let head = method == Method::HEAD;
    let mut request = client.request(method, url);
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body)?);
    }
    let response = request.send().await?;
    let status = response.status();
    if head {
        println!("{}", status.as_u16());
    }
    if !status.is_success() {
        return Err(format!("bridge returned HTTP {status}").into());
    }
    if !head {
        let bytes = response.bytes().await?;
        let mut stdout = io::stdout().lock();
        stdout.write_all(&bytes)?;
        if !bytes.is_empty() {
            stdout.write_all(b"\n")?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bridge-cli: {error}");
            ExitCode::FAILURE
        }
    }
}
