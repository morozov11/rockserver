//! Durable, account-owned Yandex Smart Home OAuth state and encrypted token operations.

use super::rows::audit;
use super::*;

/// Ciphertext and nonce required to make a read-only Yandex Smart Home request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedYandexHomeToken {
    /// AES-GCM ciphertext including its authentication tag.
    pub ciphertext: Vec<u8>,
    /// Unique AES-GCM nonce stored beside the ciphertext.
    pub nonce: Vec<u8>,
}

impl PostgresAccountStore {
    /// Creates a one-time OAuth state bound to an active Rock account for ten minutes.
    pub async fn create_yandex_home_oauth_state(
        &self,
        user_id: Uuid,
        state_hash: &SecretHash,
    ) -> Result<bool, sqlx::Error> {
        let inserted = sqlx::query(
            "INSERT INTO yandex_home_oauth_states (state_hash, user_id, expires_at) +             SELECT $1, id, now() + interval '10 minutes' FROM users WHERE id = $2 AND status = 'active'",
        )
        .bind(state_hash.as_bytes())
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Consumes one valid OAuth state and returns its account owner exactly once.
    pub async fn consume_yandex_home_oauth_state(
        &self,
        state_hash: &SecretHash,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        sqlx::query_scalar(
            "DELETE FROM yandex_home_oauth_states s USING users u +             WHERE s.state_hash = $1 AND s.user_id = u.id AND s.expires_at > now() AND u.status = 'active' +             RETURNING s.user_id",
        )
        .bind(state_hash.as_bytes())
        .fetch_optional(&self.pool)
        .await
    }

    /// Stores a replacement encrypted token for the owner and restores a previously revoked link.
    pub async fn save_yandex_home_connection(
        &self,
        user_id: Uuid,
        token: &EncryptedYandexHomeToken,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query(
            "INSERT INTO yandex_home_connections (user_id, access_token_ciphertext, nonce) +             SELECT id, $2, $3 FROM users WHERE id = $1 AND status = 'active' +             ON CONFLICT (user_id) DO UPDATE SET access_token_ciphertext = EXCLUDED.access_token_ciphertext, +             nonce = EXCLUDED.nonce, updated_at = now(), revoked_at = NULL",
        )
        .bind(user_id)
        .bind(&token.ciphertext)
        .bind(&token.nonce)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 1 {
            audit(
                &mut transaction,
                Some(user_id),
                None,
                "yandex_home_connected",
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Returns a linked account's encrypted token only while both account and integration are active.
    pub async fn yandex_home_connection(
        &self,
        user_id: Uuid,
    ) -> Result<Option<EncryptedYandexHomeToken>, sqlx::Error> {
        sqlx::query_as::<_, YandexHomeConnectionRow>(
            "SELECT c.access_token_ciphertext, c.nonce FROM yandex_home_connections c +             JOIN users u ON u.id = c.user_id WHERE c.user_id = $1 AND c.revoked_at IS NULL AND u.status = 'active'",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(Into::into))
    }

    /// Reports whether the signed-in account has an active Yandex Smart Home link.
    pub async fn has_yandex_home_connection(&self, user_id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM yandex_home_connections c JOIN users u ON u.id = c.user_id +             WHERE c.user_id = $1 AND c.revoked_at IS NULL AND u.status = 'active')",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
    }

    /// Revokes the linked external authorization without deleting its audit-safe row.
    pub async fn revoke_yandex_home_connection(&self, user_id: Uuid) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE yandex_home_connections SET revoked_at = now(), updated_at = now() +             WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 1 {
            audit(
                &mut transaction,
                Some(user_id),
                None,
                "yandex_home_disconnected",
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(updated.rows_affected() == 1)
    }
}

#[derive(sqlx::FromRow)]
struct YandexHomeConnectionRow {
    access_token_ciphertext: Vec<u8>,
    nonce: Vec<u8>,
}

impl From<YandexHomeConnectionRow> for EncryptedYandexHomeToken {
    fn from(row: YandexHomeConnectionRow) -> Self {
        Self {
            ciphertext: row.access_token_ciphertext,
            nonce: row.nonce,
        }
    }
}
