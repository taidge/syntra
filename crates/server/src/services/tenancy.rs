//! Tenant boundaries are enforced by the shared ACL and credential services.
//! Unassigned users share the default namespace; tenant-bound users cannot use
//! memberships or shares to cross into another tenant or the default namespace.
use diesel::prelude::*;
use diesel::sql_types::{Integer, Nullable, Text};
use horme::RunQueryDsl;
use syntra_core::error::AppError;

#[derive(diesel::QueryableByName)]
struct Membership {
    #[diesel(sql_type=Nullable<Text>)]
    tenant_id: Option<String>,
    #[diesel(sql_type=Nullable<Integer>)]
    is_active: Option<i32>,
}
const MEMBERSHIP_SQL: &str = "SELECT users.tenant_id, tenants.is_active FROM users LEFT JOIN tenants ON tenants.id = users.tenant_id WHERE users.id = ?";
fn active(row: Membership) -> Result<Option<String>, AppError> {
    if row.tenant_id.is_some() && row.is_active != Some(1) {
        return Err(AppError::Forbidden("Tenant is inactive or missing".into()));
    }
    Ok(row.tenant_id)
}
pub fn user_tenant(conn: &mut SqliteConnection, user: &str) -> Result<Option<String>, AppError> {
    let row = diesel::RunQueryDsl::get_result::<Membership>(
        diesel::sql_query(MEMBERSHIP_SQL).bind::<Text, _>(user),
        conn,
    )?;
    active(row)
}
pub fn ensure_same_tenant(
    conn: &mut SqliteConnection,
    first: &str,
    second: &str,
) -> Result<(), AppError> {
    if user_tenant(conn, first)? != user_tenant(conn, second)? {
        return Err(AppError::Forbidden(
            "Cross-tenant access is not permitted".into(),
        ));
    }
    Ok(())
}
pub async fn user_tenant_async(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    user: &str,
) -> Result<Option<String>, AppError> {
    let row = diesel::sql_query(MEMBERSHIP_SQL)
        .bind::<Text, _>(user)
        .get_result::<Membership>(conn)
        .await?;
    active(row)
}
pub async fn ensure_same_tenant_async(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    first: &str,
    second: &str,
) -> Result<(), AppError> {
    if user_tenant_async(conn, first).await? != user_tenant_async(conn, second).await? {
        return Err(AppError::Forbidden(
            "Cross-tenant access is not permitted".into(),
        ));
    }
    Ok(())
}
pub async fn ensure_active(
    state: &crate::state::AppState,
    user: &str,
) -> Result<Option<String>, AppError> {
    user_tenant_async(&mut state.db.clone(), user).await
}
