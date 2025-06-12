####################################################################################################
## Base image
####################################################################################################
FROM clux/muslrust:stable AS chef
USER root
WORKDIR /app
RUN cargo install cargo-chef

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
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
