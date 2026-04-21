// @generated automatically by Diesel CLI.
// Run `diesel print-schema` after running migrations to regenerate.

diesel::table! {
    use diesel::sql_types::*;
    use diesel::pg::sql_types::*;

    users (id) {
        id -> Uuid,
        email -> Text,
        password_hash -> Text,
        created_at -> Timestamptz,
    }
}
