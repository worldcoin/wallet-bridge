# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`world-id-bridge` (binary): an end-to-end encrypted relay between the World ID SDK (IDKit) and the World App. The bridge stores opaque `{iv, payload}` blobs in Redis with a TTL and brokers a state machine between two clients that never talk directly. The Rust server intentionally has no knowledge of the encrypted contents.

## Common commands

Toolchain is pinned via `rust-toolchain.toml` to **Rust 1.88** with target `x86_64-unknown-linux-musl`. CI builds against musl; local `cargo build` defaults to the host target.

```bash
cargo fmt -- --check          # format check (CI gate)
cargo clippy --all-features   # lint (CI gate; src is `#![deny(clippy::all, clippy::pedantic, clippy::nursery)]` so warnings fail the build)
cargo build --release --locked --target x86_64-unknown-linux-musl   # CI release build
cargo run                     # local dev (needs REDIS_URL)
```

Cargo deny runs in CI (`deny.toml` restricts licenses and registries) — run locally with `cargo deny check` if dependencies change.

### Running

Requires either `REDIS_URL` **or** all of `REDIS_HOST`/`REDIS_PORT`/`REDIS_USERNAME`/`REDIS_PASSWORD` (optional `REDIS_USE_TLS=true` switches scheme to `rediss://`). `PORT` defaults to 8000. `ENVIRONMENT` is read at request-handler init time and gates a staging-only route (see Architecture). `INVITE_CODE_FLOW_ENABLED=true` turns on the invite-code endpoints (off by default — see Architecture).

```bash
docker run -d -p 6379:6379 redis
REDIS_URL=redis://localhost:6379 cargo run
```

### Tests

There are **no unit tests**. `tests/integration_test.rs` is a black-box suite that hits a running server over HTTP via libcurl. The server must already be up before invoking `cargo test`:

```bash
docker-compose -f docker-compose.test.yml up -d        # Redis on :6379
REDIS_URL=redis://localhost:6379 cargo run             # in another shell
cargo test --test integration_test                     # runs sequentially via `-- --test-threads=1` is NOT set; tests use unique UUIDs to avoid collisions
cargo test --test integration_test <name>              # run a single test by substring
```

Override target via `WALLET_BRIDGE_URL` (default `http://localhost:8000`).

## Architecture

Single-process axum server, no internal modules beyond routes:

- `src/main.rs` — env loading, JSON tracing, builds a `redis::aio::ConnectionManager` (30s connect timeout) and hands it to `server::start`.
- `src/server.rs` — wires axum, attaches the `ConnectionManager` and `OpenApi` doc as `Extension` layers, sets a 5 MiB `DefaultBodyLimit`, installs SIGTERM/Ctrl-C graceful shutdown.
- `src/routes/{request,response,system}.rs` — endpoint handlers. `aide` produces the OpenAPI spec; `axum-jsonschema` enforces request schemas derived from `schemars` types in `utils.rs`.
- `src/utils.rs` — shared types (`RequestPayload`, `RequestStatus`) and the global `EXPIRE_AFTER_SECONDS` TTL applied to every key.
- `build.rs` — bakes `STATIC_BUILD_DATE` and (best-effort) `GIT_REV` into the binary; surfaced via `GET /`.

### Redis layout (the only storage)

Three key namespaces, all with the same TTL:

- `req:<uuid>`           → encrypted request payload (set by `POST /request`, **deleted on read** via `GETDEL` in `GET /request/:id`)
- `res:<uuid>`           → encrypted response payload (set by `PUT /response/:id` or `POST /response`, **deleted on read** via `GETDEL` in `GET /response/:id`)
- `req:status:<uuid>`    → state-machine marker, `initialized` | `retrieved` | `completed`

The "one-time retrieval" guarantee is the GETDEL — both client-facing GETs atomically read-and-delete the payload in the same pipeline as a status fetch. Do not split these into separate GET + DEL calls; that reintroduces a race.

### State machine

`initialized` → `retrieved` → `completed`. Drives the polling client (IDKit) so it can show progress before the response lands. Transitions are logged with `tracing::info!` in the format `Request {id} state transition: X -> Y` — keep that log shape, ops dashboards depend on it.

### Two flows share endpoints

1. **IDKit-initiated** (canonical): `POST /request` → `GET /request/:id` (Authenticator pulls + status flips to `retrieved`) → `PUT /response/:id` (Authenticator pushes; uses Redis SET NX so duplicates return 409) → `GET /response/:id` (IDKit pulls + status cleared).
2. **Authenticator-initiated standalone**: `POST /response` mints a UUID and writes both the `res:` payload and an `initialized` status marker in one go. IDKit then `GET /response/:id`.

Both flows go through the same `GET /response/:id`, which is why it must handle "no response yet, return current status" (returns `{status, response: None}`) and "response present, return + delete" (returns `{status: completed, response: Some}`) in one handler.

### Environment-gated routes

`PUT /request/:id` is **only registered when `ENVIRONMENT=staging`** (case-insensitive, trimmed). It exists for the simulator and is intentionally absent in production. Do not unconditionally enable it. The check happens once at router-construction time, so changing `ENVIRONMENT` requires a restart.

The CORS layer in `routes/response.rs` allows `PUT` because the simulator needs it; the inline TODO flags this as deliberately permissive — don't "clean it up" without coordinating with the simulator team.

`INVITE_CODE_FLOW_ENABLED=true` (case-insensitive, default `false`) gates the invite-code endpoints. When off:
- `POST /code/redeem` is not registered (axum returns its native 404). Read at router-construction time, so flipping this var requires a restart for the route to appear or disappear.
- `POST /request` with `request_code_enabled: true` returns 503 Service Unavailable. Checked per-request, so this branch responds immediately to a flag flip.
- `GET /response/:id` is structurally unaffected — its session-nonce gate is data-driven (presence of the `req:nonce:<id>` row), and no such rows are written when the flag is off.

### Validation boundary

Handlers never decrypt or inspect payloads. `RequestPayload` only requires `iv` and `payload` strings; everything else is opaque. Treat the bridge as a dumb pipe — adding business logic here is almost always wrong.

## Conventions

- Pedantic clippy is on; new code must pass `cargo clippy --all-features` cleanly. `#[allow(...)]` is used sparingly (`unused_async` on handlers with no awaits, `needless_pass_by_value` on the redis error helper) — prefer fixing the lint over silencing it.
- All Redis errors funnel through `handle_redis_error` so they get logged and mapped to 500. Use it instead of `?`-mapping ad hoc.
- Request IDs are server-generated `Uuid::v4` — never accept a client-supplied ID for `POST /request` or `POST /response`.
- TTL (`EXPIRE_AFTER_SECONDS` in `utils.rs`) has been bounced repeatedly in recent history (900 ↔ 2700 ↔ 7200) for ad-hoc rollouts; if you change it, expect to revert. Anchor the value in a clear reason in the PR description.
