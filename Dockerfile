# The OCR models, pinned and checked (scripts/ocr-models.sh). Its own stage,
# so code changes don't download them again.
FROM rust:1.99-bookworm AS models
COPY scripts/ocr-models.sh /ocr-models.sh
RUN /ocr-models.sh /ocr-models

# Ubuntu 24.04, as on the EC2 host that runs the binary natively. The ONNX
# Runtime linked in by `ocr` needs GCC 14's libstdc++, newer than Debian
# bookworm's. The Rust toolchain is the official 1.99 image's.
FROM ubuntu:24.04 AS build
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates build-essential pkg-config libssl-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust:1.99-bookworm /usr/local/rustup /usr/local/rustup
COPY --from=rust:1.99-bookworm /usr/local/cargo /usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
WORKDIR /src
COPY . .
# Production: the BIR relay files (src/bir/ and the rest) must be present.
# `ocr` links ONNX Runtime statically (downloaded and checked by `ort`).
RUN cargo build --release --locked --features bir,ocr

FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/buwiz-cloud /usr/local/bin/buwiz-cloud
COPY --from=build /src/target/release/buwiz-sender /usr/local/bin/buwiz-sender
COPY --from=models /ocr-models /usr/local/share/buwiz/ocr-models
USER nobody
ENV HOST=0.0.0.0 PORT=8080 OCR_MODEL_DIR=/usr/local/share/buwiz/ocr-models
EXPOSE 8080
CMD ["buwiz-cloud"]
