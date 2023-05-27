####################################################################################################
## Base image
####################################################################################################
FROM rust:latest AS builder

RUN update-ca-certificates

WORKDIR /world-id-bridge

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

WORKDIR /world-id-bridge

# Copy our build
COPY --from=builder /world-id-bridge/target/release/world-id-bridge /world-id-bridge/world-id-bridge

CMD ["/world-id-bridge/world-id-bridge"]
