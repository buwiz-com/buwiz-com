FROM rust:1.99-bookworm AS build
WORKDIR /src
COPY . .
# Production: the BIR relay files (src/bir/ and the rest) must be present.
RUN cargo build --release --locked --features bir

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/buwiz-cloud /usr/local/bin/buwiz-cloud
COPY --from=build /src/target/release/buwiz-sender /usr/local/bin/buwiz-sender
USER nobody
ENV HOST=0.0.0.0 PORT=8080
EXPOSE 8080
CMD ["buwiz-cloud"]
