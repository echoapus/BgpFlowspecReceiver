FROM rust:1.98-slim AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN cargo build --locked --release

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends tcpdump ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/bgpx /usr/local/bin/bgpx
EXPOSE 179 8080
ENTRYPOINT ["bgpx"]
CMD ["--host", "0.0.0.0", "--port", "8080"]
