# Agent guide

Read `README.md` first. It defines the bridge; every change must preserve these invariants:

- **Opaque payloads.** The bridge only ever handles client-encrypted ciphertext (`iv` + `payload`). Never inspect, transform, or log payload contents.
- **Single-use.** Requests and responses are read with `GETDEL` — one retrieval, then gone.
- **No persistence.** Everything lives in Redis with a uniform TTL (`EXPIRE_AFTER_SECONDS` in `src/utils.rs`) and is auto-purged. Don't add durable storage or per-key lifetimes.

The bridge is a dumb relay. **New** functionality must stay agnostic to clients and environments — no client/app-specific branching, no environment-specific behavior. Two intentional, temporary exceptions already exist; don't extend them or add new ones like them:

- `app_overrides` — a per-`app_id` rollout workaround for the World ID app (`src/routes/request.rs`, `src/main.rs`).
- `PUT /request/:id` — enabled only when `ENVIRONMENT == "staging"` (`src/routes/request.rs`).

## Build, test, lint

Reproduce CI locally before pushing:

```bash
cargo fmt -- --check
cargo clippy --all-features          # warnings denied via crate-level attributes
cargo build --locked
cargo test                           # in-process; needs only Redis — see README "Testing"
```
