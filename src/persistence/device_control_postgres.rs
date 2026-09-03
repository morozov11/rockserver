//! PostgreSQL persistence for current device-control projections and bounded command idempotency.

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::device_control::{
    CommandLifecycle, CommandReservation, CommandResult, DeviceCapability, DeviceControlStore,
    DeviceId, DeviceManifest, DeviceStateSnapshot, Entity, EntityState, StoreError, StoreOutcome,
    Surface, Timestamp, revision_order,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// PostgreSQL implementation of the device-control persistence boundary.
#[derive(Clone, Debug)]
pub struct PostgresDeviceControlStore {
    pool: PgPool,
}
impl PostgresDeviceControlStore {
    /// Connects and applies the shared migration sequence.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        if let Err(error) = MIGRATOR.run(&pool).await {
            pool.close().await;
            return Err(error.into());
        }
        Ok(Self { pool })
    }
    /// Reuses a caller-owned migrated pool.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
    /// Closes the underlying pool.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[async_trait]
impl DeviceControlStore for PostgresDeviceControlStore {
    async fn apply_manifest(
        &self,
        user: Uuid,
        device: DeviceId,
        manifest: DeviceManifest,
    ) -> Result<StoreOutcome, StoreError> {
        manifest.validate().map_err(StoreError::validation)?;
        let payload = json(&manifest)?;
        let mut tx = self.pool.begin().await.map_err(StoreError::database)?;
        if !owned(&mut tx, user, device).await? {
            return Ok(StoreOutcome::NotOwned);
        };
        if let Some((r, current)) = sqlx::query_as::<_, (i64, Value)>(
            "SELECT revision,payload FROM device_control_manifests WHERE device_id=$1 FOR UPDATE",
        )
        .bind(device.0)
        .fetch_optional(&mut *tx)
        .await
        .map_err(StoreError::database)?
            && let Some(x) = outcome(r as u64, &current, manifest.manifest_revision, &payload)
        {
            return Ok(x);
        }
        for table in [
            "device_control_capabilities",
            "device_control_entities",
            "device_control_surfaces",
        ] {
            sqlx::query(&format!(
                "UPDATE {table} SET removed_at=now() WHERE device_id=$1 AND removed_at IS NULL"
            ))
            .bind(device.0)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::database)?;
        }
        for cap in &manifest.capabilities.items {
            let payload = json(cap)?;
            let name = cap.name();
            if let Some((old, removed)) = sqlx::query_as::<_, (Value, bool)>("SELECT payload,removed_at IS NOT NULL FROM device_control_capabilities WHERE device_id=$1 AND capability_name=$2 FOR UPDATE")
                .bind(device.0).bind(&name).fetch_optional(&mut *tx).await.map_err(StoreError::database)?
                && removed && old != payload { return Ok(StoreOutcome::Conflict); }
            let version = payload["version"].as_i64().unwrap_or(1) as i16;
            sqlx::query("INSERT INTO device_control_capabilities(device_id,capability_name,version,payload,manifest_revision) VALUES($1,$2,$3,$4,$5) ON CONFLICT(device_id,capability_name) DO UPDATE SET version=EXCLUDED.version,payload=EXCLUDED.payload,manifest_revision=EXCLUDED.manifest_revision,removed_at=NULL")
                .bind(device.0).bind(name).bind(version).bind(payload).bind(manifest.manifest_revision as i64).execute(&mut *tx).await.map_err(StoreError::database)?;
        }
        for entity in &manifest.entities {
            if !projection(
                &mut tx,
                "device_control_entities",
                "entity_id",
                device,
                &entity.entity_id,
                json(entity)?,
                manifest.manifest_revision,
            )
            .await?
            {
                return Ok(StoreOutcome::Conflict);
            }
        }
        for surface in &manifest.surfaces {
            if !projection(
                &mut tx,
                "device_control_surfaces",
                "surface_id",
                device,
                &surface.surface_id,
                json(surface)?,
                manifest.manifest_revision,
            )
            .await?
            {
                return Ok(StoreOutcome::Conflict);
            }
        }
        sqlx::query("INSERT INTO device_control_manifests(device_id,revision,payload) VALUES($1,$2,$3) ON CONFLICT(device_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload,accepted_at=now()").bind(device.0).bind(manifest.manifest_revision as i64).bind(payload).execute(&mut *tx).await.map_err(StoreError::database)?;
        tx.commit().await.map_err(StoreError::database)?;
        Ok(StoreOutcome::Accepted)
    }
    async fn load_manifest(
        &self,
        user: Uuid,
        device: DeviceId,
    ) -> Result<Option<DeviceManifest>, StoreError> {
        load_owned(&self.pool, user, device, "device_control_manifests", None).await
    }
    async fn list_entities(&self, user: Uuid, device: DeviceId) -> Result<Vec<Entity>, StoreError> {
        list_owned(
            &self.pool,
            user,
            device,
            "device_control_entities",
            "entity_id",
        )
        .await
    }
    async fn list_surfaces(
        &self,
        user: Uuid,
        device: DeviceId,
    ) -> Result<Vec<Surface>, StoreError> {
        list_owned(
            &self.pool,
            user,
            device,
            "device_control_surfaces",
            "surface_id",
        )
        .await
    }
    async fn list_capabilities(
        &self,
        user: Uuid,
        device: DeviceId,
    ) -> Result<Vec<DeviceCapability>, StoreError> {
        list_owned(
            &self.pool,
            user,
            device,
            "device_control_capabilities",
            "capability_name",
        )
        .await
    }
    async fn store_device_state(
        &self,
        user: Uuid,
        device: DeviceId,
        mut state: DeviceStateSnapshot,
    ) -> Result<StoreOutcome, StoreError> {
        let mut tx = self.pool.begin().await.map_err(StoreError::database)?;
        if !owned(&mut tx, user, device).await? {
            return Ok(StoreOutcome::NotOwned);
        };
        state.received_at = Some(now(&mut tx).await?);
        let payload = json(&state)?;
        if let Some((r,current))=sqlx::query_as::<_,(i64,Value)>("SELECT revision,payload FROM device_control_state_snapshots WHERE device_id=$1 FOR UPDATE").bind(device.0).fetch_optional(&mut *tx).await.map_err(StoreError::database)?
            && let Some(x)=state_outcome(r as u64,current,state.state_revision,&payload) { return Ok(x); }
        sqlx::query("INSERT INTO device_control_state_snapshots(device_id,revision,payload,observed_at) VALUES($1,$2,$3,$4::timestamptz) ON CONFLICT(device_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload,observed_at=EXCLUDED.observed_at,received_at=now()").bind(device.0).bind(state.state_revision as i64).bind(payload).bind(state.observed_at.as_str()).execute(&mut *tx).await.map_err(StoreError::database)?;
        tx.commit().await.map_err(StoreError::database)?;
        Ok(StoreOutcome::Accepted)
    }
    async fn load_device_state(
        &self,
        user: Uuid,
        device: DeviceId,
    ) -> Result<Option<DeviceStateSnapshot>, StoreError> {
        load_owned(
            &self.pool,
            user,
            device,
            "device_control_state_snapshots",
            None,
        )
        .await
    }
    async fn store_entity_state(
        &self,
        user: Uuid,
        device: DeviceId,
        mut state: EntityState,
    ) -> Result<StoreOutcome, StoreError> {
        let mut tx = self.pool.begin().await.map_err(StoreError::database)?;
        if !owned(&mut tx, user, device).await? {
            return Ok(StoreOutcome::NotOwned);
        };
        let entity=sqlx::query_scalar::<_,Value>("SELECT payload FROM device_control_entities WHERE device_id=$1 AND entity_id=$2 AND removed_at IS NULL FOR UPDATE").bind(device.0).bind(&state.entity_id).fetch_optional(&mut *tx).await.map_err(StoreError::database)?;
        let Some(entity) = entity else {
            return Ok(StoreOutcome::NotOwned);
        };
        state
            .validate_for(&decode::<Entity>(entity)?)
            .map_err(StoreError::validation)?;
        state.received_at = Some(now(&mut tx).await?);
        let payload = json(&state)?;
        if let Some((r,current))=sqlx::query_as::<_,(i64,Value)>("SELECT revision,payload FROM device_control_entity_states WHERE device_id=$1 AND entity_id=$2 FOR UPDATE").bind(device.0).bind(&state.entity_id).fetch_optional(&mut *tx).await.map_err(StoreError::database)?
            && let Some(x)=state_outcome(r as u64,current,state.entity_revision,&payload) { return Ok(x); }
        sqlx::query("INSERT INTO device_control_entity_states(device_id,entity_id,revision,payload,observed_at,stale_after) VALUES($1,$2,$3,$4,$5::timestamptz,$6::timestamptz) ON CONFLICT(device_id,entity_id) DO UPDATE SET revision=EXCLUDED.revision,payload=EXCLUDED.payload,observed_at=EXCLUDED.observed_at,stale_after=EXCLUDED.stale_after,received_at=now()").bind(device.0).bind(&state.entity_id).bind(state.entity_revision as i64).bind(payload).bind(state.observed_at.as_str()).bind(state.stale_after.as_str()).execute(&mut *tx).await.map_err(StoreError::database)?;
        tx.commit().await.map_err(StoreError::database)?;
        Ok(StoreOutcome::Accepted)
    }
    async fn load_entity_state(
        &self,
        user: Uuid,
        device: DeviceId,
        entity: &str,
    ) -> Result<Option<EntityState>, StoreError> {
        load_owned(
            &self.pool,
            user,
            device,
            "device_control_entity_states",
            Some(entity),
        )
        .await
    }
    async fn reserve_command(
        &self,
        user: Uuid,
        device: DeviceId,
        request: CommandReservation,
    ) -> Result<StoreOutcome, StoreError> {
        request
            .command
            .validate_at(&request.deadline_at)
            .map_err(StoreError::validation)?;
        if request.command.target.device_id != device {
            return Ok(StoreOutcome::NotOwned);
        };
        let payload = json(&request.command)?;
        let mut tx = self.pool.begin().await.map_err(StoreError::database)?;
        if !owned(&mut tx, user, device).await? {
            return Ok(StoreOutcome::NotOwned);
        };
        if let Some((owner,target,fingerprint,status,fresh))=sqlx::query_as::<_,(Uuid,Uuid,Vec<u8>,String,bool)>("SELECT user_id,target_device_id,request_fingerprint,status,created_at >= now()-interval '24 hours' FROM device_control_commands WHERE command_id=$1 FOR UPDATE").bind(request.command.command_id.0).fetch_optional(&mut *tx).await.map_err(StoreError::database)? {
            if fresh && owner==user && target==device.0 && fingerprint==request.fingerprint { return Ok(StoreOutcome::Replay); }
            if !fresh && matches!(status.as_str(),"succeeded"|"failed") { sqlx::query("DELETE FROM device_control_commands WHERE command_id=$1").bind(request.command.command_id.0).execute(&mut *tx).await.map_err(StoreError::database)?; } else { return Ok(StoreOutcome::Conflict); }
        }
        sqlx::query("INSERT INTO device_control_commands(command_id,user_id,target_device_id,request_fingerprint,request_payload,status,deadline_at) VALUES($1,$2,$3,$4,$5,'reserved',$6::timestamptz)").bind(request.command.command_id.0).bind(user).bind(device.0).bind(request.fingerprint.to_vec()).bind(payload).bind(request.deadline_at.as_str()).execute(&mut *tx).await.map_err(StoreError::database)?;
        tx.commit().await.map_err(StoreError::database)?;
        Ok(StoreOutcome::Accepted)
    }
    async fn load_command(
        &self,
        user: Uuid,
        device: DeviceId,
        id: crate::device_control::CommandId,
    ) -> Result<Option<CommandLifecycle>, StoreError> {
        let row=sqlx::query_as::<_,(Value,Option<Value>)>("SELECT c.request_payload,c.result_payload FROM device_control_commands c JOIN devices d ON d.id=c.target_device_id JOIN users u ON u.id=d.user_id WHERE c.command_id=$1 AND c.user_id=$2 AND c.target_device_id=$3 AND d.revoked_at IS NULL AND u.status='active'").bind(id.0).bind(user).bind(device.0).fetch_optional(&self.pool).await.map_err(StoreError::database)?;
        row.map(|(command, result)| {
            Ok(CommandLifecycle {
                command: decode(command)?,
                result: result.map(decode).transpose()?,
            })
        })
        .transpose()
    }
    async fn complete_command(
        &self,
        user: Uuid,
        device: DeviceId,
        result: CommandResult,
    ) -> Result<StoreOutcome, StoreError> {
        result.validate().map_err(StoreError::validation)?;
        let payload = json(&result)?;
        let status = match result.status {
            crate::device_control::CommandStatus::Succeeded => "succeeded",
            crate::device_control::CommandStatus::Failed => "failed",
        };
        let mut tx = self.pool.begin().await.map_err(StoreError::database)?;
        if !owned(&mut tx, user, device).await? {
            return Ok(StoreOutcome::NotOwned);
        };
        let existing=sqlx::query_as::<_,(String,Option<Value>)>("SELECT status,result_payload FROM device_control_commands WHERE command_id=$1 AND user_id=$2 AND target_device_id=$3 FOR UPDATE").bind(result.command_id.0).bind(user).bind(device.0).fetch_optional(&mut *tx).await.map_err(StoreError::database)?;
        let Some((current, prior)) = existing else {
            return Ok(StoreOutcome::NotOwned);
        };
        if current != "reserved" {
            return Ok(if prior == Some(payload) {
                StoreOutcome::Replay
            } else {
                StoreOutcome::Conflict
            });
        }
        sqlx::query("UPDATE device_control_commands SET status=$1,result_payload=$2,completed_at=$3::timestamptz,updated_at=now() WHERE command_id=$4").bind(status).bind(payload).bind(result.completed_at.as_str()).bind(result.command_id.0).execute(&mut *tx).await.map_err(StoreError::database)?;
        tx.commit().await.map_err(StoreError::database)?;
        Ok(StoreOutcome::Accepted)
    }
    async fn prune_commands(&self, limit: u32) -> Result<u64, StoreError> {
        Ok(sqlx::query("WITH expired AS (SELECT command_id FROM device_control_commands WHERE status IN ('succeeded','failed') AND completed_at < now()-interval '24 hours' ORDER BY completed_at,command_id LIMIT $1) DELETE FROM device_control_commands c USING expired WHERE c.command_id=expired.command_id").bind(i64::from(limit)).execute(&self.pool).await.map_err(StoreError::database)?.rows_affected())
    }
}

async fn owned(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    device: DeviceId,
) -> Result<bool, StoreError> {
    Ok(sqlx::query_scalar::<_,Uuid>("SELECT d.id FROM devices d JOIN users u ON u.id=d.user_id WHERE d.id=$1 AND d.user_id=$2 AND d.revoked_at IS NULL AND u.status='active' FOR UPDATE OF d").bind(device.0).bind(user).fetch_optional(&mut **tx).await.map_err(StoreError::database)?.is_some())
}
async fn now(tx: &mut Transaction<'_, Postgres>) -> Result<Timestamp, StoreError> {
    Timestamp::parse(
        sqlx::query_scalar::<_, String>(
            "SELECT to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')",
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(StoreError::database)?,
    )
    .map_err(StoreError::validation)
}
fn outcome<T: PartialEq>(
    accepted_revision: u64,
    accepted: &T,
    incoming_revision: u64,
    incoming: &T,
) -> Option<StoreOutcome> {
    match revision_order(
        accepted_revision,
        accepted,
        incoming_revision,
        incoming,
        None,
    ) {
        crate::device_control::RevisionOrder::Next => None,
        crate::device_control::RevisionOrder::Stale => Some(StoreOutcome::Stale),
        crate::device_control::RevisionOrder::Replay => Some(StoreOutcome::Replay),
        crate::device_control::RevisionOrder::Conflict => Some(StoreOutcome::Conflict),
        crate::device_control::RevisionOrder::Gap => Some(StoreOutcome::Resync),
    }
}
fn state_outcome(
    accepted_revision: u64,
    mut accepted: Value,
    incoming_revision: u64,
    incoming: &Value,
) -> Option<StoreOutcome> {
    accepted
        .as_object_mut()
        .and_then(|value| value.remove("received_at"));
    let mut incoming = incoming.clone();
    incoming
        .as_object_mut()
        .and_then(|value| value.remove("received_at"));
    outcome(accepted_revision, &accepted, incoming_revision, &incoming)
}
async fn projection(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    column: &str,
    device: DeviceId,
    id: &str,
    payload: Value,
    revision: u64,
) -> Result<bool, StoreError> {
    let existing=sqlx::query_as::<_,(Value,bool)>(&format!("SELECT payload,removed_at IS NOT NULL FROM {table} WHERE device_id=$1 AND {column}=$2 FOR UPDATE")).bind(device.0).bind(id).fetch_optional(&mut **tx).await.map_err(StoreError::database)?;
    if existing.is_some_and(|(old, removed)| removed && old != payload) {
        return Ok(false);
    }
    sqlx::query(&format!("INSERT INTO {table}(device_id,{column},payload,manifest_revision) VALUES($1,$2,$3,$4) ON CONFLICT(device_id,{column}) DO UPDATE SET payload=EXCLUDED.payload,manifest_revision=EXCLUDED.manifest_revision,removed_at=NULL")).bind(device.0).bind(id).bind(payload).bind(revision as i64).execute(&mut **tx).await.map_err(StoreError::database)?;
    Ok(true)
}
async fn list_owned<T: DeserializeOwned>(
    pool: &PgPool,
    user: Uuid,
    device: DeviceId,
    table: &str,
    column: &str,
) -> Result<Vec<T>, StoreError> {
    let sql = format!(
        "SELECT p.payload FROM {table} p JOIN devices d ON d.id=p.device_id JOIN users u ON u.id=d.user_id WHERE p.device_id=$1 AND d.user_id=$2 AND d.revoked_at IS NULL AND u.status='active' AND p.removed_at IS NULL ORDER BY p.{column}"
    );
    sqlx::query_scalar::<_, Value>(&sql)
        .bind(device.0)
        .bind(user)
        .fetch_all(pool)
        .await
        .map_err(StoreError::database)?
        .into_iter()
        .map(decode)
        .collect()
}
async fn load_owned<T: DeserializeOwned>(
    pool: &PgPool,
    user: Uuid,
    device: DeviceId,
    table: &str,
    entity: Option<&str>,
) -> Result<Option<T>, StoreError> {
    let sql = if entity.is_some() {
        format!(
            "SELECT p.payload FROM {table} p JOIN device_control_entities e ON e.device_id=p.device_id AND e.entity_id=p.entity_id AND e.removed_at IS NULL JOIN devices d ON d.id=p.device_id JOIN users u ON u.id=d.user_id WHERE p.device_id=$1 AND p.entity_id=$2 AND d.user_id=$3 AND d.revoked_at IS NULL AND u.status='active'"
        )
    } else {
        format!(
            "SELECT p.payload FROM {table} p JOIN devices d ON d.id=p.device_id JOIN users u ON u.id=d.user_id WHERE p.device_id=$1 AND d.user_id=$2 AND d.revoked_at IS NULL AND u.status='active'"
        )
    };
    let mut q = sqlx::query_scalar::<_, Value>(&sql).bind(device.0);
    if let Some(entity) = entity {
        q = q.bind(entity).bind(user)
    } else {
        q = q.bind(user)
    }
    q.fetch_optional(pool)
        .await
        .map_err(StoreError::database)?
        .map(decode)
        .transpose()
}
fn json<T: Serialize>(value: &T) -> Result<Value, StoreError> {
    serde_json::to_value(value).map_err(StoreError::database)
}
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, StoreError> {
    serde_json::from_value(value).map_err(StoreError::database)
}
