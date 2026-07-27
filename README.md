# Message Bridge (previously Wallet Bridge)

The message bridge is a **dumb**, environment and client agnostic relay of arbitrary messages. It lets two parties share an arbitrary message where parties can gossip a 
symmetric key off-band.

The bridge is made so that it cannot eavesdrop on any messages. The bridge expects only ciphertext. **All clients MUST encrypt all payloads before submitting to the bridge.**.

- **Importantly** this bridge is completely agnostic to any client or environment, it simply relays messages, hence it implements no logic or opinions related to client handling.
- All messages received by this bridge are temporarily held for delivery, but are automatically purged after a period of time. This service has and SHOULD NOT have _persisting_ storage.

## Use Case: World ID Protocol

The bridge is currently used in the [World ID Protocol](https://github.com/worldcoin/world-id-protocol). The most used path is for RPs to request proofs from users (to their Authenticators) and for Authenticators to send back proofs.

### Example Flow

```mermaid
sequenceDiagram
IDKit ->> Bridge: POST /request
Bridge ->> IDKit: <id>
IDKit ->> Bridge: Poll for updates GET /response/:id
Authenticator ->> Bridge: GET /request/:id
Bridge ->> Authenticator: <request>
Authenticator ->> Bridge: PUT /response/:id
IDKit ->> Bridge: Poll for updates GET /response/:id
Bridge ->> IDKit: <response>
```

- `POST /request`: Called by IDKit. Initializes a proof verification request.
- `GET /request/:id`: Called by Authenticator. Used to fetch the proof verification request. One time use.
- `PUT /response/:id`: Called by Authenticator. Used to send the proof back to the application.
- `GET /response/:id`: Called by IDKit. Continuous pulling to fetch the status of the request and the response if available. Response can only be retrieved once.

### Standalone Response Flow

This flow allows a client to send a `/response` without first generating a `/request` first.

```mermaid
sequenceDiagram
    participant ClientA
    participant Bridge
    participant ClientB

    ClientA->>Bridge: POST /response (payload)
    Bridge->>ClientA: 201 CREATED {request_id}
    ClientB->>Bridge: GET /response/:request_id
    Bridge->>ClientB: 200 OK {response}
```

## Local Development

An easy way to run is using a Dockerized Redis:

```
docker run -d -p 6379:6379 redis
```

When building the Dockerfile locally remember to specify the `--platform=linux/amd64` flag.

## Testing

Integration tests use Redis (dockerized in `docker-compose.test.yml`), which can be run with:

```bash
docker-compose -f docker-compose.test.yml up
cargo test
```
