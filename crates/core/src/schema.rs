diesel::table! {
    users (id) {
        id -> Text,
        username -> Text,
        email -> Text,
        password_hash -> Text,
        display_name -> Nullable<Text>,
        avatar_url -> Nullable<Text>,
        role -> Text,
        storage_used -> BigInt,
        storage_quota -> Nullable<BigInt>,
        created_at -> Text,
        updated_at -> Text,
        last_login_at -> Nullable<Text>,
    }
}

diesel::table! {
    files (id) {
        id -> Text,
        name -> Text,
        path -> Text,
        mime_type -> Nullable<Text>,
        size -> BigInt,
        owner_id -> Text,
        parent_id -> Nullable<Text>,
        is_folder -> Bool,
        is_trashed -> Bool,
        trashed_at -> Nullable<Text>,
        created_at -> Text,
        updated_at -> Text,
        room_id -> Nullable<Text>,
        thumbnail_path -> Nullable<Text>,
        sort_order -> Integer,
    }
}

diesel::table! {
    file_properties (id) {
        id -> Text,
        file_id -> Text,
        prop_key -> Text,
        prop_value -> Text,
    }
}

diesel::table! {
    file_read_status (id) {
        id -> Text,
        user_id -> Text,
        file_id -> Text,
        read_at -> Text,
    }
}

diesel::table! {
    documents (id) {
        id -> Text,
        file_id -> Text,
        doc_type -> Text,
        content -> Nullable<Text>,
        version -> Integer,
        created_at -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    versions (id) {
        id -> Text,
        document_id -> Text,
        version_number -> Integer,
        content -> Nullable<Text>,
        created_by -> Text,
        label -> Nullable<Text>,
        created_at -> Text,
    }
}

diesel::table! {
    shares (id) {
        id -> Text,
        file_id -> Text,
        shared_by -> Text,
        shared_with -> Nullable<Text>,
        permission -> Text,
        public_token -> Nullable<Text>,
        expires_at -> Nullable<Text>,
        created_at -> Text,
        shared_with_group -> Nullable<Text>,
        password_hash -> Nullable<Text>,
    }
}

diesel::table! {
    comments (id) {
        id -> Text,
        document_id -> Text,
        user_id -> Text,
        content -> Text,
        parent_id -> Nullable<Text>,
        is_resolved -> Bool,
        position -> Nullable<Text>,
        created_at -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    notifications (id) {
        id -> Text,
        user_id -> Text,
        notification_type -> Text,
        title -> Text,
        message -> Nullable<Text>,
        link -> Nullable<Text>,
        is_read -> Bool,
        created_at -> Text,
    }
}

diesel::table! {
    activity_logs (id) {
        id -> Text,
        user_id -> Text,
        action -> Text,
        resource_type -> Text,
        resource_id -> Text,
        details -> Nullable<Text>,
        created_at -> Text,
        ip_address -> Nullable<Text>,
        user_agent -> Nullable<Text>,
    }
}

// ---------------------------------------------------------------------------
// Phase 1: Rooms, Groups, Tags, Favorites, Recent
// ---------------------------------------------------------------------------

diesel::table! {
    rooms (id) {
        id -> Text,
        name -> Text,
        description -> Nullable<Text>,
        room_type -> Text,
        owner_id -> Text,
        is_archived -> Bool,
        created_at -> Text,
        updated_at -> Text,
        logo_url -> Nullable<Text>,
        color -> Nullable<Text>,
        storage_quota -> Nullable<BigInt>,
        auto_delete_days -> Nullable<Integer>,
        auto_delete_enabled -> Bool,
    }
}

diesel::table! {
    room_members (id) {
        id -> Text,
        room_id -> Text,
        user_id -> Text,
        role -> Text,
        joined_at -> Text,
    }
}

diesel::table! {
    groups (id) {
        id -> Text,
        name -> Text,
        description -> Nullable<Text>,
        created_by -> Text,
        created_at -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    group_members (id) {
        id -> Text,
        group_id -> Text,
        user_id -> Text,
        added_at -> Text,
    }
}

diesel::table! {
    tags (id) {
        id -> Text,
        name -> Text,
        color -> Nullable<Text>,
        owner_id -> Text,
        created_at -> Text,
    }
}

diesel::table! {
    file_tags (id) {
        id -> Text,
        file_id -> Text,
        tag_id -> Text,
        created_at -> Text,
    }
}

diesel::table! {
    favorites (id) {
        id -> Text,
        user_id -> Text,
        file_id -> Text,
        created_at -> Text,
    }
}

diesel::table! {
    recent_files (id) {
        id -> Text,
        user_id -> Text,
        file_id -> Text,
        accessed_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Security
// ---------------------------------------------------------------------------

diesel::table! {
    sessions (id) {
        id -> Text,
        user_id -> Text,
        token_hash -> Text,
        ip_address -> Nullable<Text>,
        user_agent -> Nullable<Text>,
        is_active -> Bool,
        created_at -> Text,
        expires_at -> Text,
        last_activity_at -> Text,
    }
}

diesel::table! {
    totp_secrets (id) {
        id -> Text,
        user_id -> Text,
        secret -> Text,
        is_enabled -> Bool,
        backup_codes -> Nullable<Text>,
        created_at -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    password_policies (id) {
        id -> Text,
        min_length -> Integer,
        require_uppercase -> Bool,
        require_lowercase -> Bool,
        require_digit -> Bool,
        require_special -> Bool,
        max_age_days -> Nullable<Integer>,
        updated_by -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    ip_restrictions (id) {
        id -> Text,
        ip_range -> Text,
        restriction_type -> Text,
        description -> Nullable<Text>,
        created_by -> Text,
        created_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 3: Webhooks, Branding, SMTP
// ---------------------------------------------------------------------------

diesel::table! {
    webhooks (id) {
        id -> Text,
        name -> Text,
        url -> Text,
        secret -> Nullable<Text>,
        events -> Text,
        is_active -> Bool,
        created_by -> Text,
        created_at -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    webhook_deliveries (id) {
        id -> Text,
        webhook_id -> Text,
        event -> Text,
        payload -> Text,
        response_status -> Nullable<Integer>,
        response_body -> Nullable<Text>,
        delivered_at -> Text,
        success -> Bool,
    }
}

diesel::table! {
    branding_settings (id) {
        id -> Text,
        logo_url -> Nullable<Text>,
        favicon_url -> Nullable<Text>,
        app_name -> Text,
        primary_color -> Nullable<Text>,
        custom_css -> Nullable<Text>,
        updated_by -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    smtp_settings (id) {
        id -> Text,
        host -> Text,
        port -> Integer,
        username -> Nullable<Text>,
        password -> Nullable<Text>,
        from_email -> Text,
        from_name -> Text,
        use_tls -> Bool,
        updated_by -> Text,
        updated_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 4: Templates and invitations
// ---------------------------------------------------------------------------

diesel::table! {
    templates (id) {
        id -> Text,
        name -> Text,
        description -> Nullable<Text>,
        doc_type -> Text,
        content -> Nullable<Text>,
        thumbnail_url -> Nullable<Text>,
        is_global -> Bool,
        created_by -> Text,
        created_at -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    invitations (id) {
        id -> Text,
        email -> Text,
        role -> Text,
        invited_by -> Text,
        token -> Text,
        message -> Nullable<Text>,
        is_accepted -> Bool,
        accepted_at -> Nullable<Text>,
        expires_at -> Text,
        created_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 5: Chunked Upload
// ---------------------------------------------------------------------------

diesel::table! {
    upload_sessions (id) {
        id -> Text,
        user_id -> Text,
        file_name -> Text,
        mime_type -> Nullable<Text>,
        total_size -> BigInt,
        uploaded_size -> BigInt,
        chunk_count -> Integer,
        parent_id -> Nullable<Text>,
        room_id -> Nullable<Text>,
        status -> Text,
        created_at -> Text,
        expires_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 7: WOPI, Locking, Presence, Settings, Watermark, Client Logs,
//          Integrations, Conversion
// ---------------------------------------------------------------------------

diesel::table! {
    file_locks (id) {
        id -> Text,
        file_id -> Text,
        locked_by -> Text,
        lock_token -> Text,
        lock_type -> Text,
        expires_at -> Text,
        created_at -> Text,
    }
}

diesel::table! {
    user_presence (id) {
        id -> Text,
        document_id -> Text,
        user_id -> Text,
        session_id -> Nullable<Text>,
        cursor_position -> Nullable<Text>,
        is_active -> Bool,
        last_seen_at -> Text,
        joined_at -> Text,
    }
}

diesel::table! {
    user_settings (id) {
        id -> Text,
        user_id -> Text,
        setting_key -> Text,
        setting_value -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    watermark_settings (id) {
        id -> Text,
        text -> Nullable<Text>,
        opacity -> Float,
        font_size -> Integer,
        rotation -> Integer,
        is_enabled -> Bool,
        updated_by -> Text,
        updated_at -> Text,
    }
}

diesel::table! {
    client_logs (id) {
        id -> Text,
        user_id -> Nullable<Text>,
        session_id -> Nullable<Text>,
        level -> Text,
        message -> Text,
        stack_trace -> Nullable<Text>,
        user_agent -> Nullable<Text>,
        url -> Nullable<Text>,
        created_at -> Text,
    }
}

diesel::table! {
    conversion_jobs (id) {
        id -> Text,
        user_id -> Text,
        source_file_id -> Text,
        target_format -> Text,
        status -> Text,
        output_path -> Nullable<Text>,
        error_message -> Nullable<Text>,
        created_at -> Text,
        completed_at -> Nullable<Text>,
    }
}

// ---------------------------------------------------------------------------
// Phase 8: Password Resets
// ---------------------------------------------------------------------------

diesel::table! {
    password_reset_tokens (id) {
        id -> Text,
        user_id -> Text,
        token -> Text,
        expires_at -> Text,
        used -> Bool,
        created_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 9: Notification Preferences, File Versions
// ---------------------------------------------------------------------------

diesel::table! {
    notification_preferences (id) {
        id -> Text,
        user_id -> Text,
        event_type -> Text,
        enabled -> Bool,
    }
}

diesel::table! {
    file_versions (id) {
        id -> Text,
        file_id -> Text,
        version_number -> Integer,
        path -> Text,
        size -> BigInt,
        created_by -> Text,
        created_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 10: Operation Log
// ---------------------------------------------------------------------------

diesel::table! {
    operation_log (id) {
        id -> Text,
        document_id -> Text,
        user_id -> Text,
        op_type -> Text,
        op_data -> Text,
        version -> Integer,
        created_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 11: API Keys, Login Attempts
// ---------------------------------------------------------------------------

diesel::table! {
    api_keys (id) {
        id -> Text,
        user_id -> Text,
        name -> Text,
        key_hash -> Text,
        key_prefix -> Text,
        permissions -> Text,
        last_used_at -> Nullable<Text>,
        expires_at -> Nullable<Text>,
        is_active -> Bool,
        created_at -> Text,
    }
}

diesel::table! {
    login_attempts (id) {
        id -> Text,
        ip_address -> Text,
        email -> Text,
        success -> Bool,
        attempted_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Phase 12: Document Signatures
// ---------------------------------------------------------------------------

diesel::table! {
    document_signatures (id) {
        id -> Text,
        document_id -> Text,
        user_id -> Text,
        signature_data -> Text,
        signed_hash -> Text,
        signed_at -> Text,
        is_valid -> Bool,
    }
}

// ---------------------------------------------------------------------------
// Phase 13: Multi-Tenant
// ---------------------------------------------------------------------------

diesel::table! {
    tenants (id) {
        id -> Text,
        name -> Text,
        domain -> Nullable<Text>,
        owner_id -> Nullable<Text>,
        plan -> Text,
        max_users -> Integer,
        max_storage_bytes -> BigInt,
        is_active -> Integer,
        settings -> Nullable<Text>,
        created_at -> Text,
        updated_at -> Text,
    }
}

// ---------------------------------------------------------------------------
// Joinable declarations
// ---------------------------------------------------------------------------

diesel::joinable!(files -> users (owner_id));
diesel::joinable!(documents -> files (file_id));
diesel::joinable!(versions -> documents (document_id));
diesel::joinable!(comments -> documents (document_id));
diesel::joinable!(notifications -> users (user_id));
diesel::joinable!(activity_logs -> users (user_id));
diesel::joinable!(rooms -> users (owner_id));
diesel::joinable!(files -> rooms (room_id));
diesel::joinable!(tags -> users (owner_id));
diesel::joinable!(file_tags -> files (file_id));
diesel::joinable!(file_tags -> tags (tag_id));
diesel::joinable!(favorites -> users (user_id));
diesel::joinable!(favorites -> files (file_id));
diesel::joinable!(recent_files -> files (file_id));
diesel::joinable!(sessions -> users (user_id));
diesel::joinable!(webhook_deliveries -> webhooks (webhook_id));
diesel::joinable!(templates -> users (created_by));
diesel::joinable!(upload_sessions -> users (user_id));
diesel::joinable!(file_locks -> files (file_id));
diesel::joinable!(user_presence -> documents (document_id));
diesel::joinable!(conversion_jobs -> users (user_id));
diesel::joinable!(conversion_jobs -> files (source_file_id));
diesel::joinable!(password_reset_tokens -> users (user_id));
diesel::joinable!(notification_preferences -> users (user_id));
diesel::joinable!(file_versions -> files (file_id));
diesel::joinable!(api_keys -> users (user_id));
diesel::joinable!(document_signatures -> documents (document_id));
diesel::joinable!(file_properties -> files (file_id));
diesel::joinable!(file_read_status -> files (file_id));

diesel::allow_tables_to_appear_in_same_query!(
    users,
    files,
    documents,
    versions,
    shares,
    comments,
    notifications,
    activity_logs,
    rooms,
    room_members,
    groups,
    group_members,
    tags,
    file_tags,
    favorites,
    recent_files,
    sessions,
    totp_secrets,
    password_policies,
    ip_restrictions,
    webhooks,
    webhook_deliveries,
    branding_settings,
    smtp_settings,
    templates,
    invitations,
    upload_sessions,
    file_locks,
    user_presence,
    user_settings,
    watermark_settings,
    client_logs,
    conversion_jobs,
    password_reset_tokens,
    notification_preferences,
    file_versions,
    operation_log,
    api_keys,
    login_attempts,
    file_properties,
    file_read_status,
    document_signatures,
    tenants,
);
