# ---- web build ----
FROM node:22-slim AS web
WORKDIR /src/web
COPY web/package.json web/tsconfig.json ./
RUN npm install --no-audit --no-fund
COPY web/src ./src
RUN npm run build

# ---- server build ----
FROM rust:1.79-slim AS server
WORKDIR /src/server
COPY server/Cargo.toml ./
# pre-fetch deps with a dummy main for layer caching
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src
COPY server/src ./src
RUN touch src/main.rs && cargo build --release

# ---- runtime ----
FROM debian:bookworm-slim
RUN useradd -m app
WORKDIR /app
COPY --from=server /src/server/target/release/sysml-blocks-server /app/
COPY --from=web /src/web/dist /app/web
ENV MODELS_DIR=/models WEB_ROOT=/app/web PORT=8080
EXPOSE 8080
USER app
ENTRYPOINT ["/app/sysml-blocks-server"]
