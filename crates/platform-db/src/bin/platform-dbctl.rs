//! `platform-dbctl`: the administrator's tool for the control database.
//!
//! - `bootstrap`: create the owner and runtime roles and the database
//!   (`BP_ADMIN_DATABASE_URL`, `BP_DATABASE_NAME`, `BP_OWNER_PASSWORD`, `BP_APP_PASSWORD`).
//! - `migrate`: apply the migrations as the owner (`BP_MIGRATION_DATABASE_URL`).
//! - `drop`: drop a database (`BP_ADMIN_DATABASE_URL`, `BP_DATABASE_NAME`); load runs use it.
//!
//! The server and the worker never migrate: they run as the runtime role, which cannot.

use std::time::Duration;

use anyhow::Context as _;

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .with_context(|| format!("{name} is required"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("bootstrap") => {
            let database = required("BP_DATABASE_NAME")?;
            platform_db::bootstrap::bootstrap(
                &required("BP_ADMIN_DATABASE_URL")?,
                &database,
                &required("BP_OWNER_PASSWORD")?,
                &required("BP_APP_PASSWORD")?,
            )
            .await
            .map_err(|e| anyhow::anyhow!(platform_db::chain(&e)))
            .context("bootstrapping roles and the database")?;
            println!(
                "bootstrap ok: database {database}, roles {} and {}",
                platform_db::bootstrap::OWNER_ROLE,
                platform_db::bootstrap::RUNTIME_ROLE
            );
        }
        Some("migrate") => {
            let url = required("BP_MIGRATION_DATABASE_URL")?;
            let pool = platform_db::connect_lazy(&url, &platform_db::PoolSettings::default())
                .map_err(|e| anyhow::anyhow!(platform_db::chain(&e)))?;
            tokio::time::timeout(Duration::from_secs(60), platform_db::MIGRATOR.run(&pool))
                .await
                .context("migrations exceeded 60 s")?
                .context("applying migrations")?;
            pool.close().await;
            println!("migrate ok");
        }
        _ => anyhow::bail!(
            "usage: platform-dbctl bootstrap|migrate (configuration from the environment)"
        ),
    }
    Ok(())
}
