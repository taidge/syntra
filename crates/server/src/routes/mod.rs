use std::sync::Arc;

use salvo::prelude::*;

use crate::handlers;
use crate::middleware::auth::jwt_auth_middleware;
use crate::middleware::ip_filter::IpFilter;
use crate::middleware::rate_limit::RateLimiter;
use crate::middleware::security_headers::SecurityHeaders;
use crate::state::AppState;

/// Build the complete Salvo router tree.
pub fn create_router(state: Arc<AppState>) -> Router {
    // Middleware that injects AppState into every request depot.
    let state_injector = StateInjector(state.clone());

    // Edge rate limiter (runs before auth): keys by client IP and protects
    // every route, including unauthenticated ones.
    let rate_limiter = RateLimiter::new(
        state.config.rate_limit.max_requests,
        state.config.rate_limit.window_seconds,
        state.config.rate_limit.enabled,
        state.config.rate_limit.trust_forwarded_headers,
    );

    // Per-user rate limiter (runs after JWT auth): keys by the authenticated
    // user id so multiple users behind one NAT/proxy do not share a quota.
    let user_rate_limiter = RateLimiter::new(
        state.config.rate_limit.max_requests,
        state.config.rate_limit.window_seconds,
        state.config.rate_limit.enabled,
        state.config.rate_limit.trust_forwarded_headers,
    );

    Router::new()
        .hoop(state_injector)
        .hoop(IpFilter)
        .hoop(rate_limiter)
        .hoop(SecurityHeaders)
        .push(
            Router::with_path("api/v1")
                // --- Public routes (no auth required) ---
                .push(auth_routes())
                .push(health_routes())
                .push(capabilities_routes())
                .push(wopi_routes())
                .push(invitation_public_routes())
                .push(client_log_public_routes())
                .push(share_public_routes())
                // --- Protected routes (auth required) ---
                .push(
                    Router::new()
                        .hoop(jwt_auth_middleware)
                        .hoop(user_rate_limiter)
                        .push(auth_protected_routes())
                        .push(file_routes())
                        .push(folder_routes())
                        .push(document_routes())
                        .push(share_routes())
                        .push(comment_routes())
                        .push(search_routes())
                        .push(notification_routes())
                        .push(admin_routes())
                        .push(ws_routes())
                        .push(room_routes())
                        .push(group_routes())
                        .push(tag_routes())
                        .push(favorite_routes())
                        .push(recent_routes())
                        .push(trash_routes())
                        .push(session_routes())
                        .push(totp_routes())
                        .push(webhook_routes())
                        .push(template_routes())
                        .push(invitation_routes())
                        .push(upload_routes())
                        .push(quota_routes())
                        .push(settings_routes())
                        .push(presence_routes())
                        .push(file_lock_routes())
                        .push(convert_routes())
                        .push(notification_pref_routes())
                        .push(batch_routes())
                        .push(api_key_routes())
                        .push(ai_routes()),
                ),
        )
        // Robots.txt at root
        .push(Router::with_path("robots.txt").get(handlers::capabilities::robots_txt))
        // --- SPA fallback: serve static files and index.html ---
        .get(handlers::spa::spa_fallback)
        .push(Router::with_path("{**rest}").get(handlers::spa::spa_fallback))
}

// ---------------------------------------------------------------------------
// State injector handler
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct StateInjector(Arc<AppState>);

#[handler]
impl StateInjector {
    async fn handle(&self, depot: &mut Depot) {
        depot.inject(self.0.clone());
    }
}

// ---------------------------------------------------------------------------
// Route groups
// ---------------------------------------------------------------------------

fn auth_routes() -> Router {
    Router::with_path("auth")
        .push(Router::with_path("register").post(handlers::auth::register))
        .push(Router::with_path("login").post(handlers::auth::login))
        .push(Router::with_path("forgot-password").post(handlers::auth::forgot_password))
        .push(Router::with_path("reset-password").post(handlers::auth::reset_password))
}

fn auth_protected_routes() -> Router {
    Router::with_path("auth")
        .push(
            Router::with_path("me")
                .get(handlers::auth::me)
                .put(handlers::auth::update_me),
        )
        .push(Router::with_path("password").put(handlers::auth::change_password))
}

fn health_routes() -> Router {
    Router::with_path("health").get(handlers::health::health_check)
}

fn file_routes() -> Router {
    Router::with_path("files")
        .get(handlers::files::list_files)
        .push(Router::with_path("upload").post(handlers::files::upload_file))
        .push(
            Router::with_path("{id}")
                .get(handlers::files::get_file)
                .put(handlers::files::update_file)
                .delete(handlers::files::delete_file)
                .push(Router::with_path("move").post(handlers::files::move_file))
                .push(Router::with_path("download").get(handlers::files::download_file))
                .push(Router::with_path("order").put(handlers::files::set_file_order))
                .push(Router::with_path("tags").get(handlers::tags::list_file_tags))
                .push(
                    Router::with_path("versions")
                        .get(handlers::file_versions::list_versions)
                        .post(handlers::file_versions::create_version),
                )
                .push(
                    Router::with_path("properties")
                        .get(handlers::file_properties::list_properties)
                        .push(
                            Router::with_path("{key}")
                                .put(handlers::file_properties::set_property)
                                .delete(handlers::file_properties::delete_property),
                        ),
                ),
        )
}

fn folder_routes() -> Router {
    Router::with_path("folders")
        .post(handlers::folders::create_folder)
        .push(
            Router::with_path("{id}")
                .get(handlers::folders::get_folder)
                .put(handlers::folders::update_folder)
                .delete(handlers::folders::delete_folder)
                .push(Router::with_path("children").get(handlers::folders::list_folder_children))
                .push(Router::with_path("path").get(handlers::folders::get_folder_path)),
        )
}

fn document_routes() -> Router {
    Router::with_path("documents")
        .post(handlers::documents::create_document)
        .push(
            Router::with_path("{id}")
                .get(handlers::documents::get_document)
                .put(handlers::documents::update_document)
                .delete(handlers::documents::delete_document)
                .push(Router::with_path("content").get(handlers::documents::get_document_content))
                .push(
                    Router::with_path("export/{format}").post(handlers::documents::export_document),
                )
                .push(Router::with_path("versions").get(handlers::documents::list_versions))
                .push(Router::with_path("versions/diff").get(handlers::documents::diff_versions))
                .push(
                    Router::with_path("restore/{version_id}")
                        .post(handlers::documents::restore_version),
                )
                .push(
                    Router::with_path("comments")
                        .get(handlers::comments::list_comments)
                        .post(handlers::comments::create_comment),
                )
                .push(Router::with_path("sign").post(handlers::signatures::sign_document))
                .push(
                    Router::with_path("signatures")
                        .get(handlers::signatures::list_signatures)
                        .push(
                            Router::with_path("verify")
                                .get(handlers::signatures::verify_signatures),
                        ),
                ),
        )
}

/// Public (unauthenticated) share access by token.
fn share_public_routes() -> Router {
    Router::with_path("public/shares/{token}").get(handlers::shares::access_public_share)
}

fn share_routes() -> Router {
    Router::with_path("shares")
        .post(handlers::shares::create_share)
        .push(Router::with_path("file/{file_id}").get(handlers::shares::list_file_shares))
        .push(
            Router::with_path("{id}")
                .get(handlers::shares::get_share)
                .put(handlers::shares::update_share)
                .delete(handlers::shares::delete_share),
        )
}

fn comment_routes() -> Router {
    Router::with_path("comments").push(
        Router::with_path("{id}")
            .put(handlers::comments::update_comment)
            .delete(handlers::comments::delete_comment)
            .push(Router::with_path("resolve").post(handlers::comments::resolve_comment)),
    )
}

fn search_routes() -> Router {
    Router::with_path("search").get(handlers::search::search)
}

fn notification_routes() -> Router {
    Router::with_path("notifications")
        .get(handlers::notifications::list_notifications)
        .push(Router::with_path("read-all").put(handlers::notifications::mark_all_read))
        .push(
            Router::with_path("{id}")
                .delete(handlers::notifications::delete_notification)
                .push(Router::with_path("read").put(handlers::notifications::mark_read)),
        )
}

fn admin_routes() -> Router {
    Router::with_path("admin")
        .push(Router::with_path("stats").get(handlers::admin::get_stats))
        .push(Router::with_path("users").get(handlers::admin::list_users))
        .push(Router::with_path("users/import").post(handlers::admin::import_users))
        .push(Router::with_path("users/disabled").get(handlers::admin::list_disabled_users))
        .push(Router::with_path("users/stats").get(handlers::admin::user_status_stats))
        .push(Router::with_path("activity").get(handlers::admin::get_activity))
        .push(
            Router::with_path("ip-restrictions")
                .get(handlers::ip_restrictions::list_ip_restrictions)
                .post(handlers::ip_restrictions::create_ip_restriction)
                .push(
                    Router::with_path("{id}")
                        .get(handlers::ip_restrictions::get_ip_restriction)
                        .delete(handlers::ip_restrictions::delete_ip_restriction),
                ),
        )
        .push(
            Router::with_path("branding")
                .get(handlers::branding::get_branding)
                .put(handlers::branding::update_branding)
                .delete(handlers::branding::reset_branding),
        )
        .push(
            Router::with_path("backup")
                .get(handlers::backup::list_backups)
                .post(handlers::backup::create_backup)
                .push(Router::with_path("restore").post(handlers::backup::restore_backup))
                .push(
                    Router::with_path("{filename}/download").get(handlers::backup::download_backup),
                ),
        )
        .push(
            Router::with_path("password-policy")
                .get(handlers::password_policy::get_policy)
                .put(handlers::password_policy::update_policy),
        )
        .push(
            Router::with_path("smtp")
                .get(handlers::smtp::get_smtp)
                .put(handlers::smtp::update_smtp)
                .push(Router::with_path("test").post(handlers::smtp::test_smtp)),
        )
        .push(Router::with_path("quota/{user_id}").get(handlers::quota::get_user_quota))
        .push(Router::with_path("quota").put(handlers::quota::set_user_quota))
        .push(
            Router::with_path("watermark")
                .get(handlers::watermark::get_watermark)
                .put(handlers::watermark::update_watermark)
                .delete(handlers::watermark::disable_watermark),
        )
        .push(
            Router::with_path("client-logs")
                .get(handlers::client_log::list_client_logs)
                .delete(handlers::client_log::clear_client_logs),
        )
        .push(
            Router::with_path("integrations")
                .get(handlers::integrations::list_integrations)
                .push(
                    Router::with_path("{service_name}")
                        .get(handlers::integrations::get_integration)
                        .put(handlers::integrations::update_integration)
                        .delete(handlers::integrations::delete_integration),
                ),
        )
        .push(
            Router::with_path("monitoring")
                .get(handlers::monitoring::get_monitoring)
                .push(Router::with_path("documents").get(handlers::monitoring::get_document_stats)),
        )
        .push(
            Router::with_path("users/{id}")
                .put(handlers::admin::update_user)
                .push(Router::with_path("force-logout").post(handlers::admin::force_logout_user))
                .push(Router::with_path("disable").put(handlers::admin::disable_user))
                .push(Router::with_path("activate").put(handlers::admin::activate_user)),
        )
        .push(
            Router::with_path("tenants")
                .get(handlers::tenants::list_tenants)
                .post(handlers::tenants::create_tenant)
                .push(
                    Router::with_path("{id}")
                        .get(handlers::tenants::get_tenant)
                        .put(handlers::tenants::update_tenant)
                        .delete(handlers::tenants::deactivate_tenant)
                        .push(Router::with_path("users").get(handlers::tenants::list_tenant_users))
                        .push(Router::with_path("stats").get(handlers::tenants::tenant_stats)),
                ),
        )
}

fn ws_routes() -> Router {
    Router::with_path("ws")
        .push(Router::with_path("collab/{document_id}").goal(handlers::ws::collab_ws))
        .push(Router::with_path("notifications").goal(handlers::ws_notifications::notifications_ws))
}

fn room_routes() -> Router {
    Router::with_path("rooms")
        .get(handlers::rooms::list_rooms)
        .post(handlers::rooms::create_room)
        .push(
            Router::with_path("{id}")
                .get(handlers::rooms::get_room)
                .put(handlers::rooms::update_room)
                .delete(handlers::rooms::delete_room)
                .push(Router::with_path("archive").post(handlers::rooms::toggle_archive))
                .push(
                    Router::with_path("members")
                        .get(handlers::rooms::list_members)
                        .post(handlers::rooms::add_member),
                )
                .push(Router::with_path("members/{user_id}").delete(handlers::rooms::remove_member))
                .push(Router::with_path("files").get(handlers::rooms::list_room_files))
                .push(Router::with_path("logo").put(handlers::rooms::update_room_logo))
                .push(Router::with_path("color").put(handlers::rooms::update_room_color))
                .push(Router::with_path("quota").put(handlers::rooms::set_room_quota))
                .push(Router::with_path("lifetime").put(handlers::rooms::set_room_lifetime)),
        )
}

fn group_routes() -> Router {
    Router::with_path("groups")
        .get(handlers::groups::list_groups)
        .post(handlers::groups::create_group)
        .push(
            Router::with_path("{id}")
                .get(handlers::groups::get_group)
                .put(handlers::groups::update_group)
                .delete(handlers::groups::delete_group)
                .push(
                    Router::with_path("members")
                        .get(handlers::groups::list_members)
                        .post(handlers::groups::add_member),
                )
                .push(
                    Router::with_path("members/{user_id}").delete(handlers::groups::remove_member),
                ),
        )
}

fn tag_routes() -> Router {
    Router::with_path("tags")
        .get(handlers::tags::list_tags)
        .post(handlers::tags::create_tag)
        .push(
            Router::with_path("assign")
                .post(handlers::tags::assign_tag)
                .delete(handlers::tags::unassign_tag),
        )
        .push(
            Router::with_path("{id}")
                .put(handlers::tags::update_tag)
                .delete(handlers::tags::delete_tag),
        )
}

fn favorite_routes() -> Router {
    Router::with_path("favorites")
        .get(handlers::favorites::list_favorites)
        .post(handlers::favorites::add_favorite)
        .push(Router::with_path("{file_id}").delete(handlers::favorites::remove_favorite))
}

fn recent_routes() -> Router {
    Router::with_path("recent")
        .get(handlers::recent::list_recent)
        .delete(handlers::recent::clear_recent)
}

fn trash_routes() -> Router {
    Router::with_path("trash")
        .get(handlers::trash::list_trashed)
        .delete(handlers::trash::empty_trash)
        .push(Router::with_path("count").get(handlers::trash::trash_count))
        .push(
            Router::with_path("{id}")
                .delete(handlers::trash::permanently_delete)
                .push(Router::with_path("restore").post(handlers::trash::restore_file)),
        )
}

fn session_routes() -> Router {
    Router::with_path("sessions")
        .get(handlers::sessions::list_sessions)
        .delete(handlers::sessions::revoke_all_sessions)
        .push(Router::with_path("current").get(handlers::sessions::get_current_session))
        .push(Router::with_path("{id}").delete(handlers::sessions::revoke_session))
}

fn totp_routes() -> Router {
    Router::with_path("totp")
        .push(Router::with_path("setup").post(handlers::totp::setup_totp))
        .push(Router::with_path("verify").post(handlers::totp::verify_totp))
        .push(Router::with_path("disable").post(handlers::totp::disable_totp))
        .push(Router::with_path("status").get(handlers::totp::totp_status))
}

fn webhook_routes() -> Router {
    Router::with_path("webhooks")
        .get(handlers::webhooks::list_webhooks)
        .post(handlers::webhooks::create_webhook)
        .push(
            Router::with_path("{id}")
                .get(handlers::webhooks::get_webhook)
                .put(handlers::webhooks::update_webhook)
                .delete(handlers::webhooks::delete_webhook)
                .push(Router::with_path("deliveries").get(handlers::webhooks::list_deliveries)),
        )
}

fn template_routes() -> Router {
    Router::with_path("templates")
        .get(handlers::templates::list_templates)
        .post(handlers::templates::create_template)
        .push(
            Router::with_path("{id}")
                .get(handlers::templates::get_template)
                .put(handlers::templates::update_template)
                .delete(handlers::templates::delete_template)
                .push(
                    Router::with_path("create-document")
                        .post(handlers::templates::create_document_from_template),
                ),
        )
}

fn invitation_routes() -> Router {
    Router::with_path("invitations")
        .get(handlers::invitations::list_invitations)
        .post(handlers::invitations::create_invitation)
        .push(
            Router::with_path("{id}")
                .get(handlers::invitations::get_invitation)
                .delete(handlers::invitations::cancel_invitation),
        )
}

fn invitation_public_routes() -> Router {
    Router::with_path("invitations").push(
        Router::with_path("accept/{token}")
            .post(handlers::invitations::accept_invitation_with_depot),
    )
}

fn upload_routes() -> Router {
    Router::with_path("upload/sessions")
        .post(handlers::upload::create_upload_session)
        .push(
            Router::with_path("{id}")
                .delete(handlers::upload::cancel_upload)
                .push(Router::with_path("chunks").post(handlers::upload::upload_chunk))
                .push(Router::with_path("complete").post(handlers::upload::complete_upload)),
        )
}

fn quota_routes() -> Router {
    Router::with_path("quota").get(handlers::quota::get_my_quota)
}

fn capabilities_routes() -> Router {
    Router::with_path("capabilities").get(handlers::capabilities::get_capabilities)
}

fn wopi_routes() -> Router {
    Router::with_path("wopi")
        .push(Router::with_path("discovery").get(handlers::wopi::wopi_discovery))
        .push(Router::with_path("capabilities").get(handlers::wopi::wopi_capabilities))
        .push(
            Router::with_path("host/{doc_kind}/{mode}")
                .get(handlers::wopi::wopi_host_page)
                .post(handlers::wopi::wopi_host_page),
        )
        .push(
            Router::with_path("files/{file_id}")
                .get(handlers::wopi::check_file_info)
                .post(handlers::wopi::wopi_file_operation)
                .push(
                    Router::with_path("contents")
                        .get(handlers::wopi::get_file_contents)
                        .post(handlers::wopi::put_file_contents),
                ),
        )
}

fn client_log_public_routes() -> Router {
    Router::with_path("client-log").post(handlers::client_log::submit_client_logs)
}

fn settings_routes() -> Router {
    Router::with_path("settings")
        .get(handlers::settings::list_settings)
        .delete(handlers::settings::delete_all_settings)
        .push(
            Router::with_path("{key}")
                .get(handlers::settings::get_setting)
                .put(handlers::settings::set_setting)
                .delete(handlers::settings::delete_setting),
        )
}

fn presence_routes() -> Router {
    Router::with_path("documents/{doc_id}/presence")
        .get(handlers::presence::list_presence)
        .post(handlers::presence::join_presence)
        .delete(handlers::presence::leave_presence)
        .push(Router::with_path("heartbeat").post(handlers::presence::heartbeat))
}

fn file_lock_routes() -> Router {
    Router::with_path("files/{file_id}/lock")
        .get(handlers::file_locks::check_lock)
        .post(handlers::file_locks::acquire_lock)
        .delete(handlers::file_locks::release_lock)
        .push(Router::with_path("refresh").post(handlers::file_locks::refresh_lock))
}

fn convert_routes() -> Router {
    Router::with_path("convert")
        .get(handlers::convert::list_conversions)
        .post(handlers::convert::create_conversion)
        .push(
            Router::with_path("{id}")
                .get(handlers::convert::get_conversion)
                .push(Router::with_path("download").get(handlers::convert::download_conversion)),
        )
}

fn notification_pref_routes() -> Router {
    Router::with_path("notification-preferences")
        .get(handlers::notification_prefs::list_preferences)
        .push(Router::with_path("{event_type}").put(handlers::notification_prefs::set_preference))
}

fn batch_routes() -> Router {
    Router::with_path("files/batch")
        .push(Router::with_path("delete").post(handlers::batch::batch_delete))
        .push(Router::with_path("move").post(handlers::batch::batch_move))
        .push(Router::with_path("tag").post(handlers::batch::batch_tag))
        .push(Router::with_path("download").post(handlers::batch::batch_download))
        .push(Router::with_path("mark-read").post(handlers::batch::batch_mark_read))
        .push(Router::with_path("duplicate").post(handlers::batch::batch_duplicate))
}

fn ai_routes() -> Router {
    Router::with_path("ai").push(Router::with_path("assist").post(handlers::ai::assist))
}

fn api_key_routes() -> Router {
    Router::with_path("api-keys")
        .get(handlers::api_keys::list_api_keys)
        .post(handlers::api_keys::create_api_key)
        .push(
            Router::with_path("{id}")
                .put(handlers::api_keys::update_api_key)
                .delete(handlers::api_keys::revoke_api_key),
        )
}
