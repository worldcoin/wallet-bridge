use dotenvy::dotenv;
use hyper::Method;
use hyper::{service::service_fn, Body, Request, Response, Server};
use redis::{aio::ConnectionManager, AsyncCommands, Client};
use std::convert::Infallible;
use std::{env, net::SocketAddr};
use tower::make::Shared;

async fn handle_request(
    mut conn: ConnectionManager,
    req: Request<Body>,
) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path();
    let id = &path[1..].to_string();

    if id.is_empty() {
        return Ok(Response::builder().status(404).body(Body::empty()).unwrap());
    }

    match *req.method() {
        Method::GET => {
            let Ok(value) = conn.get::<_, String>(id).await else {
                return Ok(Response::builder().status(404).body(Body::empty()).unwrap());
            };

            Ok(Response::builder().status(200).body(value.into()).unwrap())
        }
        Method::PUT => {
            let Ok(value) = hyper::body::to_bytes(&mut req.into_body()).await else {
                return Ok(Response::builder().status(400).body(Body::empty()).unwrap());
            };

            if conn
                .set_ex::<_, _, ()>(id, value.to_vec(), 600)
                .await
                .is_err()
            {
                return Ok(Response::builder().status(500).body(Body::empty()).unwrap());
            }

            Ok(Response::builder().status(201).body(Body::empty()).unwrap())
        }
        _ => Ok(Response::builder().status(404).body(Body::empty()).unwrap()),
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    let redis = ConnectionManager::new(
        Client::open(env::var("REDIS_URL").expect("REDIS_URL not set"))
            .expect("Failed to create redis client"),
    )
    .await
    .expect("Failed to create redis connection manager");

    let make_service = Shared::new(service_fn(move |req| handle_request(redis.clone(), req)));

    let addr: SocketAddr = SocketAddr::from(([0, 0, 0, 0], 3000));

    let server = Server::bind(&addr).serve(make_service);
    println!("Listening on http://{}", addr);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
