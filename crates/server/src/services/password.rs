use horme::prelude::*;
use std::result::Result;

use syntra_core::error::AppError;
use syntra_core::models::PasswordPolicy;
use syntra_core::schema::password_policies::dsl as pp;

pub struct PasswordService;

impl PasswordService {
    /// Get the current password policy from the database.
    ///
    /// Returns sensible defaults if no policy row exists yet.
    pub async fn get_policy(
        conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    ) -> PasswordPolicy {
        match pp::password_policies
            .first::<PasswordPolicy>(conn)
            .await
            .optional()
        {
            Ok(Some(policy)) => policy,
            // Default policy enforces only a minimum length so existing
            // accounts/flows keep working; admins can opt into stricter
            // composition rules via the password-policy API.
            _ => PasswordPolicy {
                id: String::new(),
                min_length: 8,
                require_uppercase: false,
                require_lowercase: false,
                require_digit: false,
                require_special: false,
                max_age_days: None,
                updated_by: String::new(),
                updated_at: String::new(),
            },
        }
    }

    /// Validate a password against the current policy.
    ///
    /// Returns `Ok(())` if the password passes all checks, or an
    /// `AppError::Validation` with a human-readable explanation on failure.
    pub async fn validate_password(
        conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
        password: &str,
    ) -> Result<(), AppError> {
        let policy = Self::get_policy(conn).await;

        if (password.len() as i32) < policy.min_length {
            return Err(AppError::Validation(format!(
                "Password must be at least {} characters",
                policy.min_length
            )));
        }
        if policy.require_uppercase && !password.chars().any(|c| c.is_uppercase()) {
            return Err(AppError::Validation(
                "Password must contain an uppercase letter".into(),
            ));
        }
        if policy.require_lowercase && !password.chars().any(|c| c.is_lowercase()) {
            return Err(AppError::Validation(
                "Password must contain a lowercase letter".into(),
            ));
        }
        if policy.require_digit && !password.chars().any(|c| c.is_ascii_digit()) {
            return Err(AppError::Validation("Password must contain a digit".into()));
        }
        if policy.require_special && !password.chars().any(|c| !c.is_alphanumeric()) {
            return Err(AppError::Validation(
                "Password must contain a special character".into(),
            ));
        }

        Ok(())
    }
}
