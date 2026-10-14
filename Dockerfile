FROM rust:1.99-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --bin ai-usage-exporter

FROM alpine:3.24 AS codex-download
RUN apk add --no-cache curl
ARG TARGETARCH
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) arch=x86_64; checksum=f479424eca092484dc40d87ae28c44f4cc40234a60045d6131e493800d814a30 ;; \
      arm64) arch=aarch64; checksum=5cda6182bd94c3a30f2eb63a495489ebf7f691fddb14d70f48c6c1a5071b6cde ;; \
      *) echo 'Unsupported target architecture' >&2; exit 1 ;; \
    esac; \
    name="codex-$arch-unknown-linux-musl"; \
    curl --fail --location --retry 3 --max-time 120 \
      "https://github.com/openai/codex/releases/download/rust-v0.153.4/$name.tar.gz" \
      --output /tmp/codex.tar.gz; \
    printf '%s  %s\n' "$checksum" /tmp/codex.tar.gz | sha256sum -c -; \
    tar -xzf /tmp/codex.tar.gz -C /tmp "$name"; \
    cp "/tmp/$name" /codex; chmod 755 /codex

FROM alpine:3.24
RUN apk add --no-cache ca-certificates \
    && addgroup -g 10001 exporter \
    && adduser -D -u 10001 -G exporter exporter \
    && mkdir -p /var/lib/codex \
    && chown exporter:exporter /var/lib/codex

COPY --from=build /app/target/release/ai-usage-exporter /usr/local/bin/ai-usage-exporter
COPY --from=codex-download /codex /usr/local/bin/codex

ENV CODEX_HOME=/var/lib/codex \
    AI_USAGE_EXPORTER_HOST=0.0.0.0 \
    AI_USAGE_EXPORTER_PORT=9173 \
    AI_USAGE_EXPORTER_CACHE_TTL_SECONDS=300 \
    AI_USAGE_EXPORTER_FETCH_TIMEOUT_SECONDS=10

USER exporter
VOLUME ["/var/lib/codex"]
EXPOSE 9173
CMD ["ai-usage-exporter"]
