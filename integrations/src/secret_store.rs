//! Credential CRUD operations against the `credentials` Postgres table.
//!
//! This module owns the database interaction for credentials. It handles
//! both legacy Fernet-encrypted credentials (`encryption_version = 1`)
//! and new AES-256-GCM credentials (`encryption_version = 2`).

use sqlx::PgPool;
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

use crate::encryption::{EncryptionEngine, EncryptionError};

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("encryption error: {0}")]
    Encryption(#[from] EncryptionError),
    #[error("credential not found")]
    NotFound,
}

/// Row shape returned from the `credentials` table.
#[derive(Debug, sqlx::FromRow)]
pub struct CredentialRow {
    pub id: Uuid,
    pub agent_id: Option<Uuid>,
    pub name: String,
    pub provider: String,
    pub auth_type: String,
    pub encrypted_data: String,
    pub encryption_iv: Option<String>,
    pub encryption_version: i32,
    pub token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_refresh_success: Option<chrono::DateTime<chrono::Utc>>,
    pub last_refresh_failure: Option<chrono::DateTime<chrono::Utc>>,
    pub last_refresh_error: Option<String>,
    pub refresh_attempts: i32,
    pub refresh_exhausted: bool,
    pub last_fetched_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Credential store — CRUD operations on encrypted credentials.
pub struct SecretStore {
    pool: PgPool,
    engine: EncryptionEngine,
}

impl SecretStore {
    /// Create a new store with a database pool and encryption engine.
    pub fn new(pool: PgPool, engine: EncryptionEngine) -> Self {
        Self { pool, engine }
    }

    /// Encrypt and insert a new credential.
    ///
    /// Returns the new credential's UUID.
    pub async fn create(
        &self,
        agent_id: Uuid,
        name: &str,
        provider: &str,
        auth_type: &str,
        data: &serde_json::Value,
    ) -> Result<Uuid, StoreError> {
        let (ciphertext, iv) = self.engine.encrypt_json(data)?;

        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO credentials (agent_id, name, provider, auth_type, encrypted_data, encryption_iv, encryption_version)
             VALUES ($1, $2, $3, $4, $5, $6, 2)
             RETURNING id",
        )
        .bind(agent_id)
        .bind(name)
        .bind(provider)
        .bind(auth_type)
        .bind(&ciphertext)
        .bind(&iv)
        .persistent(false)
        .fetch_one(&self.pool)
        .await?;

        info!(credential_id = %id, provider = %provider, "Credential created");
        Ok(id)
    }

    /// Update an existing credential's encrypted data.
    pub async fn update_data(
        &self,
        credential_id: Uuid,
        data: &serde_json::Value,
    ) -> Result<(), StoreError> {
        let (ciphertext, iv) = self.engine.encrypt_json(data)?;

        let rows = sqlx::query(
            "UPDATE credentials
             SET encrypted_data = $1, encryption_iv = $2, encryption_version = 2
             WHERE id = $3",
        )
        .bind(&ciphertext)
        .bind(&iv)
        .bind(credential_id)
        .persistent(false)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if rows == 0 {
            return Err(StoreError::NotFound);
        }
        info!(credential_id = %credential_id, "Credential data updated");
        Ok(())
    }

    /// Update token-specific fields after a refresh.
    pub async fn update_token(
        &self,
        credential_id: Uuid,
        data: &serde_json::Value,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), StoreError> {
        let (ciphertext, iv) = self.engine.encrypt_json(data)?;

        sqlx::query(
            "UPDATE credentials SET
                encrypted_data = $1,
                encryption_iv = $2,
                encryption_version = 2,
                token_expires_at = $3,
                last_refresh_success = NOW(),
                last_refresh_failure = NULL,
                refresh_attempts = 0
             WHERE id = $4",
        )
        .bind(&ciphertext)
        .bind(&iv)
        .bind(expires_at)
        .bind(credential_id)
        .persistent(false)
        .execute(&self.pool)
        .await?;

        info!(credential_id = %credential_id, "Token refreshed");
        Ok(())
    }

    /// Record a refresh failure with cooldown tracking.
    pub async fn record_refresh_failure(
        &self,
        credential_id: Uuid,
        error_msg: &str,
        max_attempts: i32,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE credentials SET
                last_refresh_failure = NOW(),
                last_refresh_error = $1,
                refresh_attempts = refresh_attempts + 1,
                refresh_exhausted = (refresh_attempts + 1 >= $2)
             WHERE id = $3",
        )
        .bind(error_msg)
        .bind(max_attempts)
        .bind(credential_id)
        .persistent(false)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch all credentials for an agent.
    pub async fn list_by_agent(&self, agent_id: Uuid) -> Result<Vec<CredentialRow>, StoreError> {
        let rows = sqlx::query_as::<_, CredentialRow>(
            "SELECT id, agent_id, name, provider, auth_type, encrypted_data,
                    encryption_iv, encryption_version, token_expires_at,
                    last_refresh_success, last_refresh_failure, last_refresh_error, refresh_attempts, refresh_exhausted, last_fetched_at
             FROM credentials
             WHERE agent_id = $1
             ORDER BY created_at DESC",
        )
        .bind(agent_id)
        .persistent(false)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Fetch a single credential by ID.
    pub async fn get(&self, credential_id: Uuid) -> Result<CredentialRow, StoreError> {
        sqlx::query_as::<_, CredentialRow>(
            "SELECT id, agent_id, name, provider, auth_type, encrypted_data,
                    encryption_iv, encryption_version, token_expires_at,
                    last_refresh_success, last_refresh_failure, last_refresh_error, refresh_attempts, refresh_exhausted, last_fetched_at
             FROM credentials
             WHERE id = $1",
        )
        .bind(credential_id)
        .persistent(false)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StoreError::NotFound)
    }

    /// Decrypt a credential row's data.
    ///
    /// Supports both encryption versions:
    /// - Version 1: Legacy Fernet (returns error — must be migrated first)
    /// - Version 2: AES-256-GCM (current)
    pub fn decrypt_row(&self, row: &CredentialRow) -> Result<serde_json::Value, StoreError> {
        match row.encryption_version {
            2 => {
                let iv = row
                    .encryption_iv
                    .as_deref()
                    .ok_or(EncryptionError::DecryptFailed)?;
                let data = self.engine.decrypt_json(&row.encrypted_data, iv)?;
                Ok(data)
            }
            v => {
                tracing::warn!(
                    credential_id = %row.id,
                    encryption_version = v,
                    "Cannot decrypt legacy encryption version — migration required"
                );
                Err(EncryptionError::DecryptFailed.into())
            }
        }
    }

    /// Delete a credential by ID.
    pub async fn delete(&self, credential_id: Uuid) -> Result<(), StoreError> {
        let rows = sqlx::query("DELETE FROM credentials WHERE id = $1")
            .bind(credential_id)
            .persistent(false)
            .execute(&self.pool)
            .await?
            .rows_affected();

        if rows == 0 {
            return Err(StoreError::NotFound);
        }
        info!(credential_id = %credential_id, "Credential deleted");
        Ok(())
    }

    /// Fetch stale OAuth2 credentials for batch refresh.
    ///
    /// Uses `FOR UPDATE SKIP LOCKED` so multiple workers don't compete.
    /// Returns credentials where `token_expires_at` is within `margin_minutes`
    /// of expiry and the refresh is not exhausted.
    pub async fn fetch_stale_for_refresh(
        &self,
        margin_minutes: i64,
        cooldown_seconds: i64,
        batch_size: i32,
    ) -> Result<Vec<CredentialRow>, StoreError> {
        let rows = sqlx::query_as::<_, CredentialRow>(
            "SELECT id, agent_id, name, provider, auth_type, encrypted_data,
                    encryption_iv, encryption_version, token_expires_at,
                    last_refresh_success, last_refresh_failure, last_refresh_error, refresh_attempts, refresh_exhausted, last_fetched_at
             FROM credentials
             WHERE auth_type = 'oauth2'
               AND refresh_exhausted = FALSE
               AND encryption_version = 2
               AND (token_expires_at IS NULL
                    OR token_expires_at < NOW() + $1 * INTERVAL '1 minute')
               AND (last_refresh_failure IS NULL
                    OR last_refresh_failure < NOW() - $2 * INTERVAL '1 second')
             FOR UPDATE SKIP LOCKED
             LIMIT $3",
        )
        .bind(margin_minutes)
        .bind(cooldown_seconds)
        .bind(batch_size)
        .persistent(false)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Expose the encryption engine (for use by secret_resolver, oauth, etc.).
    pub fn engine(&self) -> &EncryptionEngine {
        &self.engine
    }

    /// Expose the database pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}
