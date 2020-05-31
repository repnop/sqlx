use sqlx::mysql::MySqlRow;
use sqlx::pool::PoolConnection;
use sqlx::Connect;
use sqlx::Executor;
use sqlx::MySqlConnection;
use sqlx::MySqlPool;
use sqlx::Row;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use super::{DatabaseMigrator, MigrationTransaction};

pub struct MySql {
    pub db_url: String,
}

impl MySql {
    pub fn new(db_url: String) -> Self {
        MySql {
            db_url: db_url.clone(),
        }
    }
}

struct DbUrl<'a> {
    base_url: &'a str,
    db_name: &'a str,
}

fn get_base_url<'a>(db_url: &'a str) -> Result<DbUrl> {
    let split: Vec<&str> = db_url.rsplitn(2, '/').collect();

    if split.len() != 2 {
        return Err(anyhow!("Failed to find database name in connection string"));
    }

    let db_name = split[0];
    let base_url = split[1];

    Ok(DbUrl { base_url, db_name })
}

#[async_trait]
impl DatabaseMigrator for MySql {
    fn database_type(&self) -> String {
        "MySql".to_string()
    }

    fn can_migrate_database(&self) -> bool {
        true
    }

    fn can_create_database(&self) -> bool {
        true
    }

    fn can_drop_database(&self) -> bool {
        true
    }

    fn get_database_name(&self) -> Result<String> {
        let db_url = get_base_url(&self.db_url)?;
        Ok(db_url.db_name.to_string())
    }

    async fn check_if_database_exists(&self, db_name: &str) -> Result<bool> {
        let db_url = get_base_url(&self.db_url)?;

        let base_url = db_url.base_url;

        let mut conn = MySqlConnection::connect(base_url).await?;

        let result: bool = sqlx::query(
            "select exists(SELECT 1 from INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = ?)",
        )
        .bind(db_name)
        .try_map(|row: MySqlRow| row.try_get(0))
        .fetch_one(&mut conn)
        .await
        .context("Failed to check if database exists")?;

        Ok(result)
    }

    async fn create_database(&self, db_name: &str) -> Result<()> {
        let db_url = get_base_url(&self.db_url)?;

        let base_url = db_url.base_url;

        let mut conn = MySqlConnection::connect(base_url).await?;

        sqlx::query(&format!("CREATE DATABASE `{}`", db_name))
            .execute(&mut conn)
            .await
            .with_context(|| format!("Failed to create database: {}", db_name))?;

        Ok(())
    }

    async fn drop_database(&self, db_name: &str) -> Result<()> {
        let db_url = get_base_url(&self.db_url)?;

        let base_url = db_url.base_url;

        let mut conn = MySqlConnection::connect(base_url).await?;

        sqlx::query(&format!("DROP DATABASE `{}`", db_name))
            .execute(&mut conn)
            .await
            .with_context(|| format!("Failed to drop database: {}", db_name))?;

        Ok(())
    }

    async fn create_migration_table(&self) -> Result<()> {
        let mut conn = MySqlConnection::connect(&self.db_url).await?;
        println!("Create migration table");

        sqlx::query(
            r#"
    CREATE TABLE IF NOT EXISTS __migrations (
        migration VARCHAR (255) PRIMARY KEY,
        created TIMESTAMP NOT NULL DEFAULT current_timestamp
    );
        "#,
        )
        .execute(&mut conn)
        .await
        .context("Failed to create migration table")?;

        Ok(())
    }

    async fn get_migrations(&self) -> Result<Vec<String>> {
        let mut conn = MySqlConnection::connect(&self.db_url).await?;

        let result = sqlx::query(
            r#"
            select migration from __migrations order by created
        "#,
        )
        .try_map(|row: MySqlRow| row.try_get(0))
        .fetch_all(&mut conn)
        .await
        .context("Failed to create migration table")?;

        Ok(result)
    }

    async fn begin_migration(&self) -> Result<Box<dyn MigrationTransaction>> {
        let pool = MySqlPool::new(&self.db_url)
            .await
            .context("Failed to connect to pool")?;

        let tx = pool.begin().await?;

        Ok(Box::new(MySqlMigration { transaction: tx }))
    }
}

pub struct MySqlMigration {
    transaction: sqlx::Transaction<'static, sqlx::MySql, PoolConnection<sqlx::MySql>>,
}

#[async_trait]
impl MigrationTransaction for MySqlMigration {
    async fn commit(self: Box<Self>) -> Result<()> {
        self.transaction.commit().await?;
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> Result<()> {
        self.transaction.rollback().await?;
        Ok(())
    }

    async fn check_if_applied(&mut self, migration_name: &str) -> Result<bool> {
        let result =
            sqlx::query("select exists(select migration from __migrations where migration = ?)")
                .bind(migration_name.to_string())
                .try_map(|row: MySqlRow| row.try_get(0))
                .fetch_one(&mut self.transaction)
                .await
                .context("Failed to check migration table")?;

        Ok(result)
    }

    async fn execute_migration(&mut self, migration_sql: &str) -> Result<()> {
        self.transaction.execute(migration_sql).await?;
        Ok(())
    }

    async fn save_applied_migration(&mut self, migration_name: &str) -> Result<()> {
        sqlx::query("insert into __migrations (migration) values (?)")
            .bind(migration_name.to_string())
            .execute(&mut self.transaction)
            .await
            .context("Failed to insert migration")?;
        Ok(())
    }
}
