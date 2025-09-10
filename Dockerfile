####################################################################################################
## Base image
####################################################################################################
FROM rust:1.86-slim AS builder
USER root
WORKDIR /app


RUN apt-get update && apt-get install -y \
    musl-tools \
    ca-certificates \
    libssl-dev \
    pkg-config \
    build-essential \
 && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl

RUN cargo install cargo-chef

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked --target x86_64-unknown-linux-musl

####################################################################################################
## Final image
####################################################################################################
FROM scratch

WORKDIR /app

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/world-id-bridge /app/world-id-bridge

USER 100
EXPOSE 8000
CMD ["/app/world-id-bridge"]
