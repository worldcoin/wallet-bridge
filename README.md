# Wallet Bridge

> **Warning** This project is still in early alpha.

An end-to-end encrypted bridge between the World ID SDK and World App. This bridge is used to pass zero-knowledge proofs for World ID verifications.

More details in the [docs](https://docs.worldcoin.org/further-reading/protocol-internals).

## Flow

```mermaid
sequenceDiagram
IDKit ->> Bridge: POST /request
Bridge ->> IDKit: <id>
IDKit ->> Bridge: Poll for updates GET /response/:id
WorldApp ->> Bridge: GET /request/:id
Bridge ->> WorldApp: <request>
WorldApp ->> Bridge: PUT /response/:id
IDKit ->> Bridge: Poll for updates GET /response/:id
Bridge ->> IDKit: <response>
```

```mermaid
flowchart
A[IDKit posts request /request] --> B[Request is stored in the bridge with status = initialized]
B --> C[IDKit starts polling /response/:id]
C --> D[User scans QR code with requestId & decryption key]
D --> E[App fetches request at /request/:id]
E --> F[Bridge updates status = retrieved]
F -- Status updated = retrieved --> C
F --> G[App generates proof and PUTs to /response/:id]
G --> H[Bridge stores response. One-time retrieval]
H -- Response provided --> C
```

## Endpoints

- `POST /request`: Called by IDKit. Initializes a proof verification request.
- `GET /request/:id`: Called by World App. Used to fetch the proof verification request. One time use.
- `PUT /response/:id`: Called by World App. Used to send the proof back to the application.
- `GET /response/:id`: Called by IDKit. Continuous pulling to fetch the status of the request and the response if available. Response can only be retrieved once.
