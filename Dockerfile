####################################################################################################
## Base image
####################################################################################################
FROM rust:latest AS builder

RUN update-ca-certificates

WORKDIR /app

COPY ./Cargo.toml .
COPY ./Cargo.lock .

RUN mkdir ./src && echo 'fn main() { println!("you lost the game"); }' > ./src/main.rs

RUN cargo build --release

RUN rm -rf ./src

COPY src src

RUN cargo build --release

####################################################################################################
## Final image
####################################################################################################
FROM gcr.io/distroless/cc

WORKDIR /app

COPY --from=builder /app/target/release/world-id-bridge /app/world-id-bridge

CMD ["/app/world-id-bridge"]
