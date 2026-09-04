//! Private PostgreSQL account-store operations grouped by one responsibility.

use super::*;

impl PostgresAccountStore {
    /// Atomically increments a PostgreSQL-backed rate-limit bucket when capacity remains.
    pub async fn consume_rate_limit(
        &self,
        key_hash: &SecretHash,
        bucket_started_at_rfc3339: &str,
        expires_at_rfc3339: &str,
        limit: i64,
    ) -> Result<bool, sqlx::Error> {
        let row = sqlx::query_scalar::<_, i64>(
            "INSERT INTO rate_limit_buckets (key_hash, bucket_started_at, request_count, expires_at) VALUES ($1, $2::timestamptz, 1, $3::timestamptz) \
             ON CONFLICT (key_hash, bucket_started_at) DO UPDATE SET request_count = rate_limit_buckets.request_count + 1 \
             WHERE rate_limit_buckets.request_count < $4 RETURNING request_count",
        )
        .bind(key_hash.as_bytes())
        .bind(bucket_started_at_rfc3339)
        .bind(expires_at_rfc3339)
        .bind(limit)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Consumes a database-clock fifteen-minute rate-limit bucket for an opaque endpoint key.
    pub async fn consume_rate_limit_for_minutes(
        &self,
        key_hash: &SecretHash,
        lifetime_minutes: i32,
        limit: i64,
    ) -> Result<bool, sqlx::Error> {
        let row = sqlx::query_scalar::<_, i64>(
            "INSERT INTO rate_limit_buckets (key_hash, bucket_started_at, request_count, expires_at) \
             VALUES ($1, date_trunc('minute', now()), 1, now() + ($2 * interval '1 minute')) \
             ON CONFLICT (key_hash, bucket_started_at) DO UPDATE \
             SET request_count = rate_limit_buckets.request_count + 1 \
             WHERE rate_limit_buckets.request_count < $3 RETURNING request_count",
        )
        .bind(key_hash.as_bytes())
        .bind(lifetime_minutes)
        .bind(limit)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }
}
