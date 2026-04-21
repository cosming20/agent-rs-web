DROP TRIGGER IF EXISTS conversations_touch ON conversations;
DROP TRIGGER IF EXISTS ingested_documents_touch ON ingested_documents;
DROP FUNCTION IF EXISTS touch_updated_at();

DROP TABLE IF EXISTS conversation_messages;
DROP TABLE IF EXISTS conversations;
DROP TABLE IF EXISTS ingested_documents;
DROP TABLE IF EXISTS users;
