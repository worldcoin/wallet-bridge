####################################################################################################
## Base image
####################################################################################################
FROM blackdex/rust-musl:x86_64-musl-stable AS chef
USER root
WORKDIR /app

# Install OpenSSL dev headers, MUSL tooling, and pkg-config for openssl-sys
RUN apk add --no-cache openssl-dev musl-dev pkgconfig

RUN cargo install cargo-chef

####################################################################################################
## Planner stage
####################################################################################################
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

####################################################################################################
## Builder stage
####################################################################################################
FROM chef AS builder
# Enable static linking for OpenSSL
ENV OPENSSL_STATIC=1

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

####################################################################################################
## Final image
####################################################################################################
FROM gcr.io/distroless/cc

WORKDIR /app

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/world-id-bridge /app/world-id-bridge

USER 100
EXPOSE 8000
CMD ["/app/world-id-bridge"]