-- Initial consolidated schema for agent-rs-web.
--
-- agent-rs-web owns ALL user-visible state for the platform:
--
--   * users                 — signup / login + bcrypt password
--   * ingested_documents    — per-user library (metadata only; bytes live in MinIO)
--   * conversations         — chat threads (one per active conversation)
--   * conversation_messages — per-turn messages (including which docs were attached)
--
-- agent-rs-web is the SOURCE OF TRUTH for conversations. The gRPC
-- agent-rs service is stateless about sessions — it receives history +
-- active_document_keys inline with each Ask request.
--
-- Index-state for uploaded docs lives on agent-rs (document_index_state);
-- web polls agent-rs's GetDocumentStatus RPC and mirrors the result back
-- into its own ingested_documents.ingest_status for UI display.

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ---------------------------------------------------------------------------
-- users: signup + login
-- ---------------------------------------------------------------------------

CREATE TABLE users (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT        UNIQUE NOT NULL,
    password_hash TEXT        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- `email UNIQUE` already creates a btree index implicitly. No explicit
-- idx_users_email — a second index would just double write amplification
-- for zero lookup benefit.
--
-- Email is stored verbatim and lowercased at the application layer (see
-- `auth/mod.rs::create_user` / `verify_credentials`). Using CITEXT here
-- would make the DB case-insensitive too, but the diesel integration for
-- CITEXT is awkward — application-layer normalization is enough.

-- ---------------------------------------------------------------------------
-- ingested_documents: per-user document library
--
-- Bytes live in MinIO under `minio_object_key` (format:
-- `users/{user_id_simple}/docs/{document_uuid}.{ext}`).
--
-- n_pages / n_chunks / sha256 / ingest_status are HYDRATED asynchronously
-- by polling agent-rs's GetDocumentStatus RPC while the IndexerWorker
-- processes the doc. Initial row is inserted as status='pending'.
-- ---------------------------------------------------------------------------

CREATE TABLE ingested_documents (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id           UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    minio_object_key  TEXT        NOT NULL,
    source_filename   TEXT        NOT NULL,
    content_type      TEXT        NOT NULL,
    size_bytes        BIGINT      NOT NULL
        CHECK (size_bytes > 0),
    sha256            CHAR(64)    NULL
        CHECK (sha256 IS NULL OR sha256 ~ '^[0-9a-f]{64}$'),
    n_pages           INTEGER     NULL,
    n_chunks          INTEGER     NULL,
    ingest_status     TEXT        NOT NULL DEFAULT 'pending'
        CHECK (ingest_status IN ('pending', 'indexing', 'complete', 'failed')),
    error_message     TEXT        NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_ingested_documents_user_created
    ON ingested_documents (user_id, created_at DESC);
CREATE UNIQUE INDEX ux_ingested_documents_minio_key
    ON ingested_documents (minio_object_key);
-- Per-user sha256 dedup. Partial because sha256 is hydrated AFTER indexing
-- completes (initial INSERT has sha256 NULL); without the WHERE clause the
-- first upload would block any subsequent upload due to NULL = NULL being
-- treated as NOT DISTINCT in unique indexes (Postgres <15 bug-for-bug).
CREATE UNIQUE INDEX ux_ingested_documents_user_sha256
    ON ingested_documents (user_id, sha256) WHERE sha256 IS NOT NULL;

-- ---------------------------------------------------------------------------
-- conversations: one per chat thread
-- ---------------------------------------------------------------------------

CREATE TABLE conversations (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    title      TEXT        NOT NULL DEFAULT 'New conversation'
        CHECK (LENGTH(title) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_conversations_user_updated
    ON conversations (user_id, updated_at DESC);

-- ---------------------------------------------------------------------------
-- conversation_messages: per-turn
--
-- `attached_document_ids` = ingested_documents.id values attached AT THE
-- TIME THIS MESSAGE WAS SENT. Used to reconstruct `active_keys` (current
-- turn) + `history_keys` (union of prior turns) for each Ask request.
--
-- `citations` = JSON array of { index, chunk_id, content_snippet,
-- minio_object_key, section_path, score } exactly as received from the
-- Ask stream's Final event. Keeping the raw JSON avoids a second table.
-- ---------------------------------------------------------------------------

CREATE TABLE conversation_messages (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id       UUID        NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    role                  TEXT        NOT NULL CHECK (role IN ('user', 'assistant')),
    content               TEXT        NOT NULL,
    attached_document_ids UUID[]      NOT NULL DEFAULT '{}',
    citations             JSONB       NOT NULL DEFAULT '[]'::jsonb,
    confidence            DOUBLE PRECISION
        CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
    is_grounded           BOOLEAN,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_conversation_messages_convo_time
    ON conversation_messages (conversation_id, created_at ASC);

-- ---------------------------------------------------------------------------
-- Shared helpers
-- ---------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION touch_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER ingested_documents_touch
    BEFORE UPDATE ON ingested_documents
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TRIGGER conversations_touch
    BEFORE UPDATE ON conversations
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
