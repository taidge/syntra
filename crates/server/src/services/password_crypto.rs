//! CPU-bound password work must not block an async worker or keep a DB lease.
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use syntra_core::error::AppError;

// Bound Argon2's simultaneous memory/CPU use. A worker retains its permit even
// if its HTTP caller is cancelled, so abandoned work cannot overrun this limit.
static CRYPTO_WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

pub async fn hash_password(password: String) -> Result<String, AppError> {
    hash_secret(password, "Password hashing failed").await
}

pub async fn hash_secret(
    secret: String,
    failure_context: &'static str,
) -> Result<String, AppError> {
    let permit = CRYPTO_WORKERS
        .acquire()
        .await
        .map_err(|_| AppError::Internal("Password hashing unavailable".into()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hash_secret_in_worker(&secret, failure_context)
    })
    .await
    .map_err(|_| AppError::Internal("Password hashing worker failed".into()))?
}

fn hash_secret_in_worker(secret: &str, failure_context: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| AppError::Internal(format!("{failure_context}: {error}")))
}

pub async fn hash_backup_codes(codes: Vec<String>) -> Result<Vec<String>, AppError> {
    let permit = CRYPTO_WORKERS
        .acquire()
        .await
        .map_err(|_| AppError::Internal("Password hashing unavailable".into()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        codes
            .iter()
            .map(|code| hash_secret_in_worker(code, "Failed to hash backup code"))
            .collect()
    })
    .await
    .map_err(|_| AppError::Internal("Password hashing worker failed".into()))?
}

/// Prepare the remaining backup codes off-thread. The caller must atomically
/// compare-and-set the original serialized codes before accepting this result.
pub async fn consume_backup_code(
    code: String,
    codes: Vec<String>,
) -> Result<Option<Vec<String>>, AppError> {
    let permit = CRYPTO_WORKERS
        .acquire()
        .await
        .map_err(|_| AppError::Internal("Password verification unavailable".into()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let Some(matched_index) = codes.iter().position(|stored| {
            PasswordHash::new(stored).is_ok_and(|hash| {
                Argon2::default()
                    .verify_password(code.as_bytes(), &hash)
                    .is_ok()
            }) || stored.eq_ignore_ascii_case(&code)
        }) else {
            return Ok(None);
        };
        let mut remaining = Vec::with_capacity(codes.len().saturating_sub(1));
        for (index, stored) in codes.into_iter().enumerate() {
            if index == matched_index {
                continue;
            }
            if PasswordHash::new(&stored).is_ok() {
                remaining.push(stored);
            } else {
                remaining.push(hash_secret_in_worker(
                    &stored,
                    "Failed to hash backup code",
                )?);
            }
        }
        Ok(Some(remaining))
    })
    .await
    .map_err(|_| AppError::Internal("Password verification worker failed".into()))?
}

/// Malformed stored hashes remain server errors; credential mismatches are false.
pub async fn verify_password(password: String, stored_hash: String) -> Result<bool, AppError> {
    let permit = CRYPTO_WORKERS
        .acquire()
        .await
        .map_err(|_| AppError::Internal("Password verification unavailable".into()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let parsed = PasswordHash::new(&stored_hash)
            .map_err(|_| AppError::Internal("Password hash corrupt".into()))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .map_err(|_| AppError::Internal("Password verification worker failed".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn password_round_trip_and_mismatch_on_current_thread_runtime() {
        let hash = hash_password("Password-crypto-test-42".into())
            .await
            .unwrap();
        assert!(
            verify_password("Password-crypto-test-42".into(), hash.clone())
                .await
                .unwrap()
        );
        assert!(
            !verify_password("different-password".into(), hash)
                .await
                .unwrap()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn corrupt_hash_keeps_its_internal_error_contract() {
        assert!(matches!(
            verify_password("any-password".into(), "malformed-hash".into()).await,
            Err(AppError::Internal(message)) if message == "Password hash corrupt"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn consuming_backup_code_upgrades_remaining_legacy_codes() {
        let existing_hash = hash_password("87654321".into()).await.unwrap();
        let remaining = consume_backup_code(
            "ABCDEF12".into(),
            vec!["abcdef12".into(), "12345678".into(), existing_hash.clone()],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(
            verify_password("12345678".into(), remaining[0].clone())
                .await
                .unwrap()
        );
        assert_eq!(remaining[1], existing_hash);
        assert!(
            consume_backup_code("00000000".into(), remaining)
                .await
                .unwrap()
                .is_none()
        );
    }
}
