//! Repositórios transacionais, outbox e leases com fencing por tentativa.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{
    ConnectOptions, FromRow, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, time::Duration};
use stories_core::{DomainError, EventState, PublicationKey};
use thiserror::Error;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

/// Apenas códigos controlados são persistidos como erro; mensagens de terceiros podem conter segredos.
#[derive(Debug, Clone, Copy)]
pub enum JobFailure {
    Timeout,
    Transient,
    Permanent,
}

impl JobFailure {
    fn code(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transient => "transient",
            Self::Permanent => "permanent",
        }
    }
}

#[derive(Debug, FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: Value,
    pub attempts: i32,
    pub lease_token: Uuid,
    pub lease_until: DateTime<Utc>,
}

pub struct Enqueue<'a> {
    pub kind: &'a str,
    pub key: &'a str,
    pub payload: &'a Value,
}

impl Store {
    pub async fn connect(
        url: &str,
        max_connections: u32,
        timeout: Duration,
    ) -> Result<Self, StoreError> {
        if max_connections == 0
            || max_connections > 10
            || timeout.is_zero()
            || timeout > Duration::from_secs(30)
        {
            return Err(StoreError::InvalidOptions);
        }
        let options = PgConnectOptions::from_str(url)
            .map_err(database_error)?
            .disable_statement_logging();
        let statement_timeout = timeout.as_millis().to_string();
        let pool = tokio::time::timeout(timeout, PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(timeout)
            .after_connect(move |connection, _metadata| {
                let statement_timeout = statement_timeout.clone();
                Box::pin(async move {
                    sqlx::query("SELECT set_config('statement_timeout', $1, false), set_config('lock_timeout', '2000', false)")
                        .bind(statement_timeout).execute(connection).await?;
                    Ok(())
                })
            })
            .connect_with(options)).await.map_err(|_| StoreError::Timeout)?.map_err(database_error)?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        tokio::time::timeout(Duration::from_secs(30), MIGRATOR.run(&self.pool))
            .await
            .map_err(|_| StoreError::Timeout)?
            .map_err(|_| StoreError::Migration)
    }

    pub async fn health(&self) -> Result<(), StoreError> {
        tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&self.pool),
        )
        .await
        .map_err(|_| StoreError::Timeout)?
        .map_err(database_error)?;
        Ok(())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn create_event(&self, next_job: Option<Enqueue<'_>>) -> Result<i64, StoreError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let event: i64 = sqlx::query_scalar("INSERT INTO events DEFAULT VALUES RETURNING id")
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query(
            "INSERT INTO audit_log (event_id, step, detail) VALUES ($1, 'event_created', $2)",
        )
        .bind(event)
        .bind(json!({"state": "DETECTED"}))
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if let Some(job) = next_job {
            sqlx::query("INSERT INTO jobs (kind, dedupe_key, payload) VALUES ($1, $2, $3)")
                .bind(job.kind)
                .bind(job.key)
                .bind(job.payload)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        }
        tx.commit().await.map_err(database_error)?;
        Ok(event)
    }

    /// Estado, auditoria e próximo job são confirmados em uma única transação.
    pub async fn transition(
        &self,
        event: i64,
        expected: EventState,
        next: EventState,
        reason: Option<&str>,
        next_job: Option<Enqueue<'_>>,
    ) -> Result<(), StoreError> {
        expected.advance(next, reason)?;
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let affected =
            sqlx::query("UPDATE events SET state = $1, reason = $2 WHERE id = $3 AND state = $4")
                .bind(next.as_str())
                .bind(reason)
                .bind(event)
                .bind(expected.as_str())
                .execute(&mut *tx)
                .await
                .map_err(database_error)?
                .rows_affected();
        if affected != 1 {
            return Err(StoreError::Conflict);
        }
        sqlx::query(
            "INSERT INTO audit_log (event_id, step, detail) VALUES ($1, 'state_transition', $2)",
        )
        .bind(event)
        .bind(json!({"from": expected.as_str(), "to": next.as_str(), "reason": reason}))
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if let Some(job) = next_job {
            sqlx::query("INSERT INTO jobs (kind, dedupe_key, payload) VALUES ($1, $2, $3) ON CONFLICT (dedupe_key) DO NOTHING")
                .bind(job.kind).bind(job.key).bind(job.payload).execute(&mut *tx).await.map_err(database_error)?;
        }
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub async fn enqueue(
        &self,
        job: Enqueue<'_>,
        run_at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query("INSERT INTO jobs (kind, dedupe_key, payload, run_at) VALUES ($1,$2,$3,$4) ON CONFLICT (dedupe_key) DO NOTHING")
            .bind(job.kind).bind(job.key).bind(job.payload).bind(run_at)
            .execute(&self.pool).await.map_err(database_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// O token muda ao recuperar um lease; trabalhadores antigos perdem o direito de confirmar.
    pub async fn claim(
        &self,
        lease: Duration,
        max_attempts: i32,
    ) -> Result<Option<Job>, StoreError> {
        if !(30..=3600).contains(&lease.as_secs()) || !(1..=20).contains(&max_attempts) {
            return Err(StoreError::InvalidOptions);
        }
        sqlx::query("UPDATE jobs SET dead_at = now(), last_error_code = 'lease_exhausted', lease_token = NULL, lease_until = NULL, locked_at = NULL WHERE done_at IS NULL AND dead_at IS NULL AND attempts >= $1 AND (lease_until IS NULL OR lease_until <= now())")
            .bind(max_attempts).execute(&self.pool).await.map_err(database_error)?;
        let lease_s = i64::try_from(lease.as_secs()).map_err(|_| StoreError::InvalidOptions)?;
        sqlx::query_as::<_, Job>("WITH candidate AS (SELECT id FROM jobs WHERE done_at IS NULL AND dead_at IS NULL AND run_at <= now() AND attempts < $1 AND (lease_until IS NULL OR lease_until <= now()) ORDER BY run_at, id FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE jobs SET locked_at = now(), lease_until = now() + make_interval(secs => $2::double precision), lease_token = $3, attempts = attempts + 1 FROM candidate WHERE jobs.id = candidate.id RETURNING jobs.id, kind, payload, attempts, lease_token, lease_until")
            .bind(max_attempts).bind(lease_s).bind(Uuid::new_v4())
            .fetch_optional(&self.pool).await.map_err(database_error)
    }

    pub async fn complete(&self, job: &Job) -> Result<(), StoreError> {
        let affected = sqlx::query("UPDATE jobs SET done_at = now(), lease_token = NULL, lease_until = NULL, locked_at = NULL WHERE id = $1 AND lease_token = $2 AND lease_until > now() AND done_at IS NULL AND dead_at IS NULL")
            .bind(job.id).bind(job.lease_token).execute(&self.pool).await.map_err(database_error)?.rows_affected();
        if affected != 1 {
            return Err(StoreError::LostLease);
        }
        Ok(())
    }

    pub async fn renew(&self, job: &Job, lease: Duration) -> Result<(), StoreError> {
        if !(30..=3600).contains(&lease.as_secs()) {
            return Err(StoreError::InvalidOptions);
        }
        let lease_s = i64::try_from(lease.as_secs()).map_err(|_| StoreError::InvalidOptions)?;
        let affected = sqlx::query("UPDATE jobs SET lease_until = now() + make_interval(secs => $3::double precision) WHERE id = $1 AND lease_token = $2 AND lease_until > now() AND done_at IS NULL AND dead_at IS NULL")
            .bind(job.id).bind(job.lease_token).bind(lease_s).execute(&self.pool).await.map_err(database_error)?.rows_affected();
        if affected != 1 {
            return Err(StoreError::LostLease);
        }
        Ok(())
    }

    pub async fn retry(
        &self,
        job: &Job,
        failure: JobFailure,
        max_attempts: i32,
    ) -> Result<(), StoreError> {
        if !(1..=20).contains(&max_attempts) {
            return Err(StoreError::InvalidOptions);
        }
        let dead = job.attempts >= max_attempts || matches!(failure, JobFailure::Permanent);
        let delay = retry_delay(job.attempts, job.lease_token.as_u128() as u64);
        let affected = sqlx::query("UPDATE jobs SET lease_until = NULL, lease_token = NULL, locked_at = NULL, run_at = now() + make_interval(secs => $3::double precision), last_error_code = $4, dead_at = CASE WHEN $5 THEN now() ELSE NULL END WHERE id = $1 AND lease_token = $2 AND lease_until > now() AND done_at IS NULL AND dead_at IS NULL")
            .bind(job.id).bind(job.lease_token).bind(delay).bind(failure.code()).bind(dead)
            .execute(&self.pool).await.map_err(database_error)?.rows_affected();
        if affected != 1 {
            return Err(StoreError::LostLease);
        }
        Ok(())
    }

    pub async fn reserve_publication(
        &self,
        key: PublicationKey,
        dry_run: bool,
    ) -> Result<Option<i64>, StoreError> {
        sqlx::query_scalar("INSERT INTO publications (event_id, kind, seq, status, dry_run) VALUES ($1,$2,$3,'creating',$4) ON CONFLICT (event_id, kind, seq) DO NOTHING RETURNING id")
            .bind(key.event_id()).bind(key.kind().as_str()).bind(key.sequence()).bind(dry_run)
            .fetch_optional(&self.pool).await.map_err(database_error)
    }

    pub async fn paused(&self) -> Result<bool, StoreError> {
        sqlx::query_scalar("SELECT paused FROM pipeline_control WHERE id = true")
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)
    }

    pub async fn set_paused(&self, paused: bool) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("UPDATE pipeline_control SET paused = $1, updated_at = now() WHERE id = true")
            .bind(paused)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        sqlx::query("INSERT INTO audit_log (step, detail) VALUES ('pipeline_pause', $1)")
            .bind(json!({"paused": paused}))
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }
}

/// Backoff exponencial limitado a 15 minutos, com jitter determinístico por tentativa.
pub fn retry_delay(attempt: i32, seed: u64) -> i64 {
    let exponent = u32::try_from(attempt.clamp(1, 10)).unwrap_or(1);
    let base = (1u64 << exponent).min(600);
    i64::try_from(base + seed % (base / 2 + 1)).unwrap_or(900)
}

fn database_error(error: sqlx::Error) -> StoreError {
    let code = error
        .as_database_error()
        .and_then(|value| value.code())
        .map(|value| value.into_owned());
    StoreError::Database { code }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("operação de banco falhou (SQLSTATE: {code:?})")]
    Database { code: Option<String> },
    #[error("banco excedeu o tempo limite")]
    Timeout,
    #[error("migrations falharam; conferir versão do Postgres e extensão pgvector")]
    Migration,
    #[error("opções do banco ou lease inválidas")]
    InvalidOptions,
    #[error("estado mudou durante a transação ou evento inexistente")]
    Conflict,
    #[error("lease expirou ou pertence a outra tentativa")]
    LostLease,
    #[error(transparent)]
    Domain(#[from] DomainError),
}
