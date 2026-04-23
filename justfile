# agent-rs-web local dev recipes.
#
# Prereq: docker compose, cargo-leptos (`cargo install cargo-leptos --locked`),
# diesel_cli with the postgres feature (`cargo install diesel_cli --no-default-features --features postgres`).

set dotenv-load := true

default:
    @just --list

# Bring up the web-local infra (postgres on 1055, redis on 1074).
#
# `timeout(1)` isn't a POSIX utility and isn't on macOS by default, so
# we spell the 30-second budget as a counted shell loop to stay
# portable across macOS + Linux without a coreutils dependency.
infra-up:
    docker compose up -d
    @echo "waiting up to 30s for postgres-web to be healthy..."
    @i=0; while [ $i -lt 30 ]; do \
        if docker compose exec -T postgres-web pg_isready \
            -U "$${AGENT_RS_WEB_POSTGRES_USER:-webapp}" \
            -d "$${AGENT_RS_WEB_POSTGRES_DB:-agent_rs_web}" >/dev/null 2>&1; then \
            echo "postgres-web ready"; exit 0; \
        fi; \
        sleep 1; i=$((i+1)); \
     done; \
     echo "postgres-web did NOT become healthy within 30s — check `docker logs agent-rs-web-postgres`"; \
     exit 1

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

# TRUNCATE every application table in the web-local Postgres. Keeps
# the schema + migrations ledger so you can re-seed without re-running
# `just migrations`. Destructive in the "wipes all users + docs"
# sense; idempotent structurally.
#
# Skips the prompt when `FORCE=1` so scripts / the agent-rs top-level
# `data-wipe` can cascade through without human-in-the-loop.
data-wipe:
    @if [ "${FORCE:-0}" != "1" ]; then \
        read -p "Wipe ALL data from web postgres (users, conversations, messages, ingested_documents)? [y/N] " c; \
        if [ "$c" != "y" ] && [ "$c" != "Y" ]; then echo "Aborted."; exit 1; fi; \
     fi
    @echo "→ web Postgres: truncating application tables..."
    docker compose exec -T postgres-web psql \
        -U "${AGENT_RS_WEB_POSTGRES_USER:-webapp}" \
        -d "${AGENT_RS_WEB_POSTGRES_DB:-agent_rs_web}" \
        -c 'TRUNCATE conversation_messages, conversations, ingested_documents, users RESTART IDENTITY CASCADE;'
    @echo "web data wipe complete."

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
