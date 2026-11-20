//! Metadados de credencial com compare-and-swap e barreira durável antes da renovação.

use crate::{Store, StoreError, database_error};
use serde_json::{Value, json};
use stories_core::token::{TokenFailure, TokenKind, TokenObservation};
use uuid::Uuid;

#[derive(Debug)]
pub struct TokenRecord {
    pub observation: TokenObservation,
    pub version: i64,
}
#[derive(Debug)]
pub struct TokenRefreshLease {
    pub nonce: Uuid,
}
#[derive(sqlx::FromRow)]
struct Row {
    observation: Value,
    version: i64,
}
impl TryFrom<Row> for TokenRecord {
    type Error = StoreError;
    fn try_from(row: Row) -> Result<Self, Self::Error> {
        let observation: TokenObservation =
            serde_json::from_value(row.observation).map_err(|_| StoreError::InvalidJob)?;
        if !observation.shape_valid() {
            return Err(StoreError::InvalidJob);
        }
        Ok(Self {
            observation,
            version: row.version,
        })
    }
}
fn encode(observation: &TokenObservation) -> Result<Value, StoreError> {
    if !observation.shape_valid() {
        return Err(StoreError::InvalidOptions);
    }
    serde_json::to_value(observation).map_err(|_| StoreError::InvalidOptions)
}
impl Store {
    pub async fn token_record(
        &self,
        account: &str,
        fingerprint: [u8; 32],
    ) -> Result<Option<TokenRecord>, StoreError> {
        sqlx::query_as::<_, Row>(
            "SELECT observation,version FROM token_health WHERE account_key=$1 AND fingerprint=$2",
        )
        .bind(account)
        .bind(fingerprint.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?
        .map(TryInto::try_into)
        .transpose()
    }
    pub async fn save_token_observation(
        &self,
        account: &str,
        fingerprint: [u8; 32],
        expected: Option<i64>,
        observation: &TokenObservation,
    ) -> Result<TokenRecord, StoreError> {
        if account.is_empty() || account.len() > 128 {
            return Err(StoreError::InvalidOptions);
        }
        let value = encode(observation)?;
        let row = if let Some(version) = expected {
            sqlx::query_as::<_,Row>("UPDATE token_health SET observation=$3,version=version+1,updated_at=now(),refresh_lease_token=NULL,refresh_lease_until=NULL WHERE account_key=$1 AND fingerprint=$2 AND version=$4 AND (refresh_lease_token IS NULL OR refresh_lease_until<=clock_timestamp()) RETURNING observation,version").bind(account).bind(fingerprint.as_slice()).bind(value).bind(version).fetch_optional(&self.pool).await.map_err(database_error)?
        } else {
            sqlx::query_as::<_,Row>("INSERT INTO token_health(account_key,fingerprint,observation) VALUES($1,$2,$3) ON CONFLICT(account_key,fingerprint) DO NOTHING RETURNING observation,version").bind(account).bind(fingerprint.as_slice()).bind(value).fetch_optional(&self.pool).await.map_err(database_error)?
        };
        row.ok_or(StoreError::Conflict)?.try_into()
    }
    /// Reserva única por hash. Rotação pendente já bloqueia publicação antes do HTTP.
    pub async fn begin_token_refresh(
        &self,
        account: &str,
        fingerprint: [u8; 32],
        expected: i64,
        refresh_days: u32,
    ) -> Result<Option<TokenRefreshLease>, StoreError> {
        if !(1..=30).contains(&refresh_days) {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let row=sqlx::query_as::<_,Row>("SELECT observation,version FROM token_health WHERE account_key=$1 AND fingerprint=$2 FOR UPDATE").bind(account).bind(fingerprint.as_slice()).fetch_one(&mut *tx).await.map_err(database_error)?;
        let mut record: TokenRecord = row.try_into()?;
        if record.version != expected
            || !record
                .observation
                .refresh_due(chrono::Utc::now(), i64::from(refresh_days))
        {
            return Ok(None);
        }
        record.observation.rotation_required = true;
        record.observation.failure = Some(TokenFailure::RefreshUncertain);
        let nonce = Uuid::new_v4();
        let changed=sqlx::query("UPDATE token_health SET observation=$3,version=version+1,updated_at=now(),refresh_lease_token=$4,refresh_lease_until=clock_timestamp()+interval '30 seconds',last_refresh_attempt_at=now() WHERE account_key=$1 AND fingerprint=$2 AND refresh_lease_token IS NULL")
            .bind(account).bind(fingerprint.as_slice()).bind(encode(&record.observation)?).bind(nonce).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        if changed != 1 {
            return Ok(None);
        }
        sqlx::query("INSERT INTO audit_log(step,detail) VALUES('token_refresh_started',$1)")
            .bind(json!({"account_key":account}))
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(Some(TokenRefreshLease { nonce }))
    }
    pub async fn finish_token_refresh(
        &self,
        account: &str,
        fingerprint: [u8; 32],
        lease: &TokenRefreshLease,
        observation: &TokenObservation,
    ) -> Result<TokenRecord, StoreError> {
        if observation.kind != TokenKind::Instagram {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let row=sqlx::query_as::<_,Row>("UPDATE token_health SET observation=$4,version=version+1,updated_at=now(),refresh_lease_token=NULL,refresh_lease_until=NULL WHERE account_key=$1 AND fingerprint=$2 AND refresh_lease_token=$3 AND refresh_lease_until>clock_timestamp() RETURNING observation,version")
            .bind(account).bind(fingerprint.as_slice()).bind(lease.nonce).bind(encode(observation)?).fetch_optional(&mut *tx).await.map_err(database_error)?.ok_or(StoreError::LostLease)?;
        sqlx::query("INSERT INTO audit_log(step,detail) VALUES('token_refresh_result',$1)").bind(json!({"account_key":account,"rotation_required":observation.rotation_required,"failure":observation.failure})).execute(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        row.try_into()
    }
    /// Alerta na mudança de estado ou uma vez por hora; somente categorias controladas.
    pub async fn token_alert_due(
        &self,
        account: &str,
        fingerprint: [u8; 32],
        status: &str,
    ) -> Result<bool, StoreError> {
        if !matches!(
            status,
            "unverified"
                | "valid"
                | "expiring"
                | "expired"
                | "invalid"
                | "stale"
                | "rotation_required"
        ) {
            return Err(StoreError::InvalidOptions);
        }
        Ok(sqlx::query("UPDATE token_health SET last_alert_at=now(),last_alert_status=$3 WHERE account_key=$1 AND fingerprint=$2 AND (last_alert_status IS DISTINCT FROM $3 OR last_alert_at IS NULL OR last_alert_at<now()-interval '1 hour')")
            .bind(account).bind(fingerprint.as_slice()).bind(status).execute(&self.pool).await.map_err(database_error)?.rows_affected()==1)
    }
}
