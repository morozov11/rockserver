//! Operator-only provisioning and revocation for one owner's Home Assistant bridge.

use ring::rand::{SecureRandom, SystemRandom};
use rockserver::ha_bridge::valid_selection;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::{env, error::Error};
use uuid::Uuid;

fn token() -> Result<(String, Vec<u8>), Box<dyn Error>> {
    let mut random = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| "secure random source failed")?;
    let secret = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let token = format!("ha1.{secret}");
    Ok((token.clone(), Sha256::digest(token.as_bytes()).to_vec()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().collect();
    let (Some(command), Some(owner)) = (args.get(1), args.get(2)) else {
        return Err("usage: ha_bridge_admin provision|rotate|revoke USER_UUID [weather.entity sensor.entity ...]".into());
    };
    let owner = Uuid::parse_str(owner)?;
    let pool = PgPool::connect(&env::var("DATABASE_URL")?).await?;
    sqlx::migrate!().run(&pool).await?;
    match command.as_str() {
        "provision" => {
            let Some(weather) = args.get(3) else {
                return Err("one weather.entity is required".into());
            };
            let sensors = args.get(4..).unwrap_or(&[]).to_vec();
            if !valid_selection(&sensors, weather) {
                return Err("expected at most 128 sensor.entity IDs".into());
            }
            let id = Uuid::new_v4();
            let (secret, hash) = token()?;
            let mut tx = pool.begin().await?;
            let row = sqlx::query(
                "INSERT INTO home_assistant_integrations (id, user_id, credential_hash, allowed_sensors, weather_entity) \
                 SELECT $1, id, $3, $4, $5 FROM users WHERE id = $2 AND status = 'active' \
                 ON CONFLICT (user_id) DO UPDATE SET credential_hash = EXCLUDED.credential_hash, \
                 allowed_sensors = EXCLUDED.allowed_sensors, weather_entity = EXCLUDED.weather_entity, \
                 revoked_at = NULL, epoch = NULL, revision = 0, last_payload_hash = NULL \
                 RETURNING home_assistant_integrations.id"
            ).bind(id).bind(owner).bind(hash).bind(sensors).bind(weather)
                .fetch_optional(&mut *tx).await?;
            let Some(row) = row else {
                return Err("active owner account not found".into());
            };
            let actual_id: Uuid = row.get("id");
            sqlx::query("DELETE FROM home_assistant_sensor_states WHERE integration_id = $1")
                .bind(actual_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM home_assistant_weather_states WHERE integration_id = $1")
                .bind(actual_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            // The operator captures this once in a protected secret; no token is persisted or logged.
            println!("{secret}");
        }
        "rotate" => {
            let id: Uuid = sqlx::query_scalar("SELECT id FROM home_assistant_integrations WHERE user_id = $1 AND revoked_at IS NULL")
                .bind(owner).fetch_one(&pool).await?;
            let (secret, hash) = token()?;
            sqlx::query("UPDATE home_assistant_integrations SET credential_hash = $2, epoch = NULL, revision = 0, last_payload_hash = NULL WHERE id = $1")
                .bind(id).bind(hash).execute(&pool).await?;
            println!("{secret}");
        }
        "revoke" => {
            let changed = sqlx::query("UPDATE home_assistant_integrations SET revoked_at = now(), epoch = NULL, revision = 0, last_payload_hash = NULL WHERE user_id = $1 AND revoked_at IS NULL")
                .bind(owner).execute(&pool).await?.rows_affected();
            println!("revoked={changed}");
        }
        _ => return Err("unknown command".into()),
    }
    Ok(())
}
