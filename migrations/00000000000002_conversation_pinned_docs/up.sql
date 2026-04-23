-- Per-conversation active-document pinning.
--
-- Before this migration, `send_message_action` picked the pin set
-- implicitly as "every complete document in the user's library",
-- which meant a user with 50 docs would always get 50 keys shoved at
-- the agent. Explicit pinning lets the user scope each conversation
-- to a narrower subset (e.g. "only the contract PDFs for this legal
-- thread") without touching the library contents.
--
-- Null = "not yet chosen" → UI defaults to every complete doc.
-- Empty array = "user explicitly pinned nothing" → agent runs on web
-- evidence only. The two cases MUST stay distinguishable so the UI
-- can show "(auto)" vs "(none)".

ALTER TABLE conversations
    ADD COLUMN pinned_document_ids UUID[];
