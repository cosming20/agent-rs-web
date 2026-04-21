# agent-rs-web local dev recipes.
#
# Prereq: docker compose, cargo-leptos (`cargo install cargo-leptos --locked`),
# diesel_cli with the postgres feature (`cargo install diesel_cli --no-default-features --features postgres`).

set dotenv-load := true

default:
    @just --list

# Bring up the web-local infra (postgres on 1073, redis on 1074).
infra-up:
    docker compose up -d
    @echo "waiting for postgres to be healthy..."
    @timeout 30 sh -c 'until docker compose exec -T postgres-web pg_isready -U $${AGENT_RS_WEB_POSTGRES_USER:-webapp} -d $${AGENT_RS_WEB_POSTGRES_DB:-agent_rs_web} >/dev/null 2>&1; do sleep 1; done'
    @echo "postgres-web ready"

infra-down:
    docker compose down

# DESTRUCTIVE. Wipes the users table and every session. Use only in dev.
infra-reset:
    docker compose down --volumes
    docker compose up -d

status:
    docker compose ps

# Apply diesel migrations against the web-local postgres.
migrations:
    diesel migration run --migration-dir migrations

# One-shot dev: `cargo leptos watch` with DATABASE_URL + REDIS_URL + AGENT_RS_GRPC_URL from .env.
dev:
    cargo leptos watch

# Run the CSS + WASM pipeline in release mode (without auto-reload).
build:
    cargo leptos build --release

# Format + clippy.
check:
    cargo fmt --all -- --check
    cargo clippy --features ssr --no-default-features -- -D warnings
    cargo clippy --features hydrate --no-default-features --target wasm32-unknown-unknown -- -D warnings
