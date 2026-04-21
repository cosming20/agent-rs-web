// @generated automatically by Diesel CLI.

diesel::table! {
    conversation_messages (id) {
        id -> Uuid,
        conversation_id -> Uuid,
        role -> Text,
        content -> Text,
        attached_document_ids -> Array<Nullable<Uuid>>,
        citations -> Jsonb,
        confidence -> Nullable<Float8>,
        is_grounded -> Nullable<Bool>,
        created_at -> Timestamptz,
    }
}

diesel::table! {
    conversations (id) {
        id -> Uuid,
        user_id -> Uuid,
        title -> Text,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    ingested_documents (id) {
        id -> Uuid,
        user_id -> Uuid,
        minio_object_key -> Text,
        source_filename -> Text,
        content_type -> Text,
        size_bytes -> Int8,
        #[max_length = 64]
        sha256 -> Nullable<Bpchar>,
        n_pages -> Nullable<Int4>,
        n_chunks -> Nullable<Int4>,
        ingest_status -> Text,
        error_message -> Nullable<Text>,
        created_at -> Timestamptz,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    users (id) {
        id -> Uuid,
        email -> Text,
        password_hash -> Text,
        created_at -> Timestamptz,
    }
}

diesel::joinable!(conversation_messages -> conversations (conversation_id));
diesel::joinable!(conversations -> users (user_id));
diesel::joinable!(ingested_documents -> users (user_id));

diesel::allow_tables_to_appear_in_same_query!(
    conversation_messages,
    conversations,
    ingested_documents,
    users,
);
