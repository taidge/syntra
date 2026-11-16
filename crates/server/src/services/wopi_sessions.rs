use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use contrix_core::error::AppError;
use contrix_core::schema::{sessions::dsl as s, users::dsl as u};
use diesel::prelude::*;
use horme::{OptionalExtension, RunQueryDsl};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::middleware::auth::JwtClaims;
use crate::state::AppState;

/// The current database session and account state, rather than the JWT's stale
/// role copy. Shared by HTTP/WOPI/long-lived transports when revalidating access.
pub struct ActiveJwtSession {
    pub user_id: String,
    pub role: String,
    pub session_id: String,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn validate_active_jwt_session(
    state: &AppState,
    token: &str,
) -> Result<ActiveJwtSession, AppError> {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.leeway = 0;
    let claims = jsonwebtoken::decode::<JwtClaims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(state.config.auth.jwt_secret.as_bytes()),
        &validation,
    )
    .map_err(|_| AppError::Auth("Invalid or expired access token".into()))?
    .claims;
    let mut session = validate_session_hash(state, &claims.sub, &hash_token(token)).await?;
    let jwt_expiry = DateTime::from_timestamp(
        i64::try_from(claims.exp)
            .map_err(|_| AppError::Auth("Invalid access token expiry".into()))?,
        0,
    )
    .ok_or_else(|| AppError::Auth("Invalid access token expiry".into()))?;
    session.expires_at = session.expires_at.min(jwt_expiry);
    if session.expires_at <= Utc::now() {
        return Err(AppError::Auth("Invalid or expired access token".into()));
    }
    Ok(session)
}

/// Revalidate a previously authenticated source session without retaining its
/// bearer credential in memory. Account disable and session revocation take
/// effect on the next request made with a derived WOPI token.
pub async fn validate_session_hash(
    state: &AppState,
    user_id: &str,
    token_hash: &str,
) -> Result<ActiveJwtSession, AppError> {
    crate::services::tenancy::ensure_active(state, user_id).await?;
    let mut conn = state.db.clone();
    let row = s::sessions
        .inner_join(u::users.on(s::user_id.eq(u::id)))
        .filter(s::token_hash.eq(token_hash))
        .filter(s::user_id.eq(user_id))
        .filter(s::is_active.eq(true))
        .select((s::id, u::role, s::expires_at))
        .first::<(String, String, String)>(&mut conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::Auth("Session has been revoked or expired".into()))?;
    let (session_id, role, expiry) = row;
    let expires_at = DateTime::parse_from_rfc3339(&expiry)
        .map_err(|_| AppError::Auth("Invalid session expiry".into()))?
        .with_timezone(&Utc);
    if role == "disabled" || expires_at <= Utc::now() {
        return Err(AppError::Auth("Session has been revoked or expired".into()));
    }
    Ok(ActiveJwtSession {
        user_id: user_id.to_owned(),
        role,
        session_id,
        token_hash: token_hash.to_owned(),
        expires_at,
    })
}

#[derive(Clone)]
pub struct WopiSession {
    pub user_id: String,
    pub source_token_hash: String,
    file_id: String,
    expires_at: DateTime<Utc>,
}

impl WopiSession {
    /// Retain the originating session and the derived capability's own expiry
    /// while a request reads its body or writes a replacement storage object.
    pub fn credential(&self) -> crate::middleware::auth::AuthenticatedCredential {
        crate::middleware::auth::AuthenticatedCredential {
            user_id: self.user_id.clone(),
            token_hash: self.source_token_hash.clone(),
            api_key_id: None,
            expires_at: Some(self.expires_at),
        }
    }

    /// A relative save may write another file, so revalidate the capability's
    /// bound source as well as the destination in the final write transaction.
    pub fn validate_write_in_transaction(
        &self,
        conn: &mut diesel::SqliteConnection,
    ) -> Result<(), AppError> {
        self.authorized_source_in_transaction(conn).map(|_| ())
    }

    fn authorized_source_in_transaction(
        &self,
        conn: &mut diesel::SqliteConnection,
    ) -> Result<contrix_core::models::File, AppError> {
        crate::middleware::auth::validate_credential_in_transaction(
            conn,
            &self.credential(),
            &salvo::http::Method::POST,
            &format!("/api/v1/wopi/files/{}", self.file_id),
        )?;
        let source = diesel::RunQueryDsl::first::<contrix_core::models::File>(
            contrix_core::schema::files::table.find(&self.file_id),
            conn,
        )?;
        crate::services::persistence::authorize(conn, &source, &self.user_id, "editor")?;
        Ok(source)
    }

    /// Bind a save to the source namespace and lock checked before its body
    /// was read. A relative overwrite also keeps its target's separate fence.
    pub fn write_authorization(
        &self,
        source: &contrix_core::models::File,
        lock_token: Option<&str>,
    ) -> Result<WopiWriteAuthorization, AppError> {
        if source.id != self.file_id {
            return Err(AppError::Forbidden("WOPI source binding mismatch".into()));
        }
        Ok(WopiWriteAuthorization {
            session: self.clone(),
            source_parent: source.parent_id.clone(),
            source_room: source.room_id.clone(),
            source_lock: lock_token.map(str::to_owned),
        })
    }
}

#[derive(Clone)]
pub struct WopiWriteAuthorization {
    session: WopiSession,
    source_parent: Option<String>,
    source_room: Option<String>,
    source_lock: Option<String>,
}

impl WopiWriteAuthorization {
    pub fn credential(&self) -> crate::middleware::auth::AuthenticatedCredential {
        self.session.credential()
    }

    pub fn validate_in_transaction(
        &self,
        conn: &mut diesel::SqliteConnection,
    ) -> Result<(), AppError> {
        let source = self.session.authorized_source_in_transaction(conn)?;
        if source.parent_id != self.source_parent || source.room_id != self.source_room {
            return Err(AppError::Conflict(
                "File moved; reload before saving".into(),
            ));
        }
        crate::services::persistence::check_file_lock(
            conn,
            &source.id,
            &self.session.user_id,
            self.source_lock.as_deref(),
        )
    }
}

/// In-memory short-lived WOPI session token store.
pub struct WopiSessionStore {
    sessions: Mutex<HashMap<String, WopiSession>>,
}

impl Default for WopiSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl WopiSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(&self, source: &ActiveJwtSession, file_id: &str, ttl: Duration) -> String {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let session = WopiSession {
            user_id: source.user_id.clone(),
            source_token_hash: source.token_hash.clone(),
            file_id: file_id.to_string(),
            expires_at: (Utc::now() + ttl).min(source.expires_at),
        };
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        retain_active(&mut sessions);
        sessions.insert(hash_token(&token), session);
        token
    }

    pub fn validate(&self, token: &str, file_id: &str) -> Option<WopiSession> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        retain_active(&mut sessions);
        sessions.get(&hash_token(token)).and_then(|session| {
            if session.file_id == file_id {
                Some(session.clone())
            } else {
                None
            }
        })
    }
}

fn retain_active(sessions: &mut HashMap<String, WopiSession>) {
    let now = Utc::now();
    sessions.retain(|_, session| session.expires_at > now);
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_token_for_bound_file() {
        let store = WopiSessionStore::new();
        let source = source_session();
        let token = store.create(&source, "file-1", Duration::minutes(5));

        let validated = store.validate(&token, "file-1").unwrap();
        assert_eq!(validated.user_id, "user-1");
        assert_eq!(validated.source_token_hash, "hashed-source-session");
        assert!(store.validate(&token, "file-2").is_none());
    }

    #[test]
    fn rejects_expired_token() {
        let store = WopiSessionStore::new();
        let token = store.create(&source_session(), "file-1", Duration::seconds(-1));

        assert!(store.validate(&token, "file-1").is_none());
    }

    fn source_session() -> ActiveJwtSession {
        ActiveJwtSession {
            user_id: "user-1".into(),
            role: "editor".into(),
            session_id: "session-1".into(),
            token_hash: "hashed-source-session".into(),
            expires_at: Utc::now() + Duration::minutes(1),
        }
    }

    #[test]
    fn derived_token_cannot_outlive_its_source() {
        let store = WopiSessionStore::new();
        let mut source = source_session();
        source.expires_at = Utc::now() - Duration::seconds(1);
        let token = store.create(&source, "file-1", Duration::minutes(15));
        assert!(store.validate(&token, "file-1").is_none());
    }

    #[test]
    fn retained_write_credential_preserves_both_capability_and_source_expiry() {
        let store = WopiSessionStore::new();
        let source = source_session();
        let token = store.create(&source, "file-1", Duration::minutes(15));
        let source_limited = store.validate(&token, "file-1").unwrap();
        let credential = source_limited.credential();
        assert_eq!(credential.user_id, source.user_id);
        assert_eq!(credential.token_hash, source.token_hash);
        assert_eq!(credential.expires_at, Some(source.expires_at));
        assert!(credential.api_key_id.is_none());

        let before = Utc::now();
        let token = store.create(&source, "file-1", Duration::seconds(10));
        let after = Utc::now();
        let mut capability_limited = store.validate(&token, "file-1").unwrap();
        let expiry = capability_limited.credential().expires_at.unwrap();
        assert!(expiry >= before + Duration::seconds(10));
        assert!(expiry <= after + Duration::seconds(10));
        assert!(expiry < source.expires_at);

        // A request retains this clone after initial validation. At the final
        // transaction boundary its own capability expiry must still reject it,
        // even before any lookup of the otherwise active source session.
        capability_limited.expires_at = Utc::now() - Duration::seconds(1);
        let mut conn = diesel::SqliteConnection::establish(":memory:").unwrap();
        assert!(matches!(
            capability_limited.validate_write_in_transaction(&mut conn),
            Err(AppError::Auth(_))
        ));
    }
}
