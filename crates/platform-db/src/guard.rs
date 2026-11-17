//! The runtime-role guard (F1): the server and the worker refuse to start when their
//! database role could rewrite the operation log — a superuser, the schema owner (or a
//! member of it), a role with `CREATEROLE`, `REPLICATION` or `BYPASSRLS`, or one holding
//! `UPDATE`, `DELETE` or `TRUNCATE` on `operation_events`, `CREATE` on the schema, or
//! `CREATE`/`TEMP` on the database.

use sqlx::PgPool;
use thiserror::Error;

use crate::DbError;

/// Why the runtime role was refused.
#[derive(Debug, Error)]
pub enum PrivilegeError {
    #[error("refusing to run as {role}: it is a superuser")]
    Superuser { role: String },
    #[error("refusing to run as {role}: it owns, or can act as the owner of, the control tables")]
    Owner { role: String },
    #[error("refusing to run as {role}: it has {what}")]
    TooBroad { role: String, what: &'static str },
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for PrivilegeError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

// One row of facts read straight from the catalog; the bools mirror its columns.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a 1:1 image of pg_roles and privilege checks"
)]
#[derive(Debug, sqlx::FromRow)]
struct RoleFacts {
    role: String,
    superuser: bool,
    createrole: bool,
    replication: bool,
    bypassrls: bool,
    owner: bool,
    database_create_or_temp: bool,
    schema_create: bool,
    events_rewrite: bool,
}

/// Checks the role `pool` connects as.
pub async fn ensure_runtime_privileges(pool: &PgPool) -> Result<(), PrivilegeError> {
    let facts = sqlx::query_as::<_, RoleFacts>(
        "SELECT current_user::text AS role,
                r.rolsuper AS superuser, r.rolcreaterole AS createrole,
                r.rolreplication AS replication, r.rolbypassrls AS bypassrls,
                EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                         WHERE n.nspname = 'public'
                           AND pg_has_role(current_user, c.relowner, 'MEMBER')) AS owner,
                has_database_privilege(current_database(), 'CREATE')
                    OR has_database_privilege(current_database(), 'TEMP') AS database_create_or_temp,
                has_schema_privilege('public', 'CREATE') AS schema_create,
                has_table_privilege('operation_events', 'UPDATE')
                    OR has_table_privilege('operation_events', 'DELETE')
                    OR has_table_privilege('operation_events', 'TRUNCATE') AS events_rewrite
           FROM pg_roles r WHERE r.rolname = current_user",
    )
    .fetch_one(pool)
    .await?;
    let role = facts.role.clone();
    if facts.superuser {
        return Err(PrivilegeError::Superuser { role });
    }
    if facts.owner {
        return Err(PrivilegeError::Owner { role });
    }
    let too_broad = [
        (facts.createrole, "CREATEROLE"),
        (facts.replication, "REPLICATION"),
        (facts.bypassrls, "BYPASSRLS"),
        (
            facts.database_create_or_temp,
            "CREATE or TEMP on the database",
        ),
        (facts.schema_create, "CREATE on the schema"),
        (
            facts.events_rewrite,
            "UPDATE, DELETE or TRUNCATE on operation_events",
        ),
    ];
    if let Some((_, what)) = too_broad.into_iter().find(|(has, _)| *has) {
        return Err(PrivilegeError::TooBroad { role, what });
    }
    Ok(())
}

/// [`ensure_runtime_privileges`] at process start: retries while the database is
/// unavailable (it may be starting), for at most `deadline`; a privilege problem fails at
/// once, because no amount of waiting fixes it.
pub async fn ensure_at_startup(
    pool: &PgPool,
    deadline: std::time::Duration,
) -> Result<(), PrivilegeError> {
    let until = tokio::time::Instant::now() + deadline;
    let mut delay = std::time::Duration::from_millis(200);
    loop {
        match ensure_runtime_privileges(pool).await {
            Err(PrivilegeError::Db(error))
                if error.is_unavailable() && tokio::time::Instant::now() + delay < until =>
            {
                tracing::warn!(
                    cause = crate::chain(&error),
                    "control database unavailable; retrying the privilege check"
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(std::time::Duration::from_secs(2));
            }
            other => return other,
        }
    }
}
