use diesel::SqliteConnection;
use diesel::prelude::*;

use contrix_core::error::AppError;
use contrix_core::models::PasswordPolicy;
use contrix_core::schema::password_policies::dsl as pp;

pub struct PasswordService;

impl PasswordService {
    /// Get the current password policy from the database.
    ///
    /// Returns sensible defaults if no policy row exists yet.
    pub fn get_policy(conn: &mut SqliteConnection) -> PasswordPolicy {
        match pp::password_policies
            .first::<PasswordPolicy>(conn)
            .optional()
        {
            Ok(Some(policy)) => policy,
            _ => PasswordPolicy {
                id: String::new(),
                min_length: 8,
                require_uppercase: true,
                require_lowercase: true,
                require_digit: true,
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
    pub fn validate_password(conn: &mut SqliteConnection, password: &str) -> Result<(), AppError> {
        let policy = Self::get_policy(conn);

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
