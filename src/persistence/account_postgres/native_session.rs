//! Native-session resolver adapter for the existing account store.

use super::*;

#[async_trait::async_trait]
impl NativeSessionResolver for PostgresAccountStore {
    async fn resolve_active_native_session(
        &self,
        access_hash: &SecretHash,
    ) -> Result<Option<ActiveSession>, NativeSessionLookupError> {
        self.find_active_session_by_access_hash(access_hash)
            .await
            .map_err(|_| NativeSessionLookupError)
    }
}
