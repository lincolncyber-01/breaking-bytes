//! Métricas duráveis e entrega de alertas com lease; labels nunca recebem conteúdo externo.
use crate::{Store, StoreError, database_error};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Clone, Copy)]
pub enum Metric {
    LlmDraft,
    LlmCheck,
    LlmReview,
    LlmCheckRevision,
    Render,
    Instagram(u32),
}
impl Metric {
    fn series(self) -> (&'static str, String, bool) {
        let (name, label, histogram) = match self {
            Self::LlmDraft => ("llm_latency_seconds", "draft".into(), true),
            Self::LlmCheck => ("llm_latency_seconds", "check".into(), true),
            Self::LlmReview => ("llm_latency_seconds", "review_change".into(), true),
            Self::LlmCheckRevision => ("llm_latency_seconds", "check_revision".into(), true),
            Self::Render => ("render_duration_seconds", "render".into(), true),
            Self::Instagram(code) => (
                "ig_api_errors_total",
                match code {
                    1 | 2 | 4 | 10 | 17 | 32 | 100 | 102 | 190 | 200 | 368 | 400 | 401 | 403
                    | 429 | 500 | 502 | 503 | 504 => code.to_string(),
                    _ => "other".into(),
                },
                false,
            ),
        };
        (name, label, histogram)
    }
}
#[derive(Debug, FromRow, Serialize)]
pub struct MetricRow {
    pub name: String,
    pub label: String,
    pub total: f64,
    pub samples: i64,
    pub buckets: Vec<i64>,
}
#[derive(Debug, FromRow, Serialize)]
pub struct SourceHealth {
    pub id: i32,
    pub up: bool,
    pub silence_seconds: f64,
}
#[derive(Debug, Serialize)]
pub struct OperationalSnapshot {
    pub paused: bool,
    pub articles: i64,
    pub queue_depth: i64,
    pub dead_jobs: i64,
    pub daily_budget_remaining: i64,
    pub no_news: bool,
    pub latency_high: bool,
    pub publication_failures: bool,
    pub backup_stale: bool,
    pub restore_stale: bool,
    pub sources: Vec<SourceHealth>,
    pub metrics: Vec<MetricRow>,
}
#[derive(Debug, Clone, Copy)]
pub struct AlertCondition {
    pub code: &'static str,
    pub subject: i64,
}
impl AlertCondition {
    fn valid(self) -> bool {
        matches!(
            self.code,
            "source_down"
                | "no_news"
                | "latency_high"
                | "publication_failures"
                | "token_expiring"
                | "token_invalid"
                | "budget_exhausted"
                | "dead_letter"
                | "backup_stale"
                | "restore_stale"
        ) && self.subject >= 0
    }
}
#[derive(Debug, FromRow)]
pub struct AlertLease {
    pub code: String,
    pub subject: i64,
    pub lease_token: Uuid,
    pub failures: i32,
}
#[derive(FromRow)]
struct SnapshotCounts {
    paused: bool,
    articles: i64,
    queue_depth: i64,
    dead_jobs: i64,
    published: i64,
    pending: i64,
    remote_usage: Option<i64>,
    remote_limit: Option<i64>,
    no_news: bool,
    latency_high: bool,
    publication_failures: bool,
    backup_stale: bool,
    restore_stale: bool,
}
impl Store {
    pub async fn metric(&self, metric: Metric, value: f64) -> Result<(), StoreError> {
        if !value.is_finite() || !(0.0..=31_536_000.0).contains(&value) {
            return Err(StoreError::InvalidOptions);
        }
        let (name, label, histogram) = metric.series();
        sqlx::query("SELECT record_metric($1,$2,$3,$4)")
            .bind(name)
            .bind(label)
            .bind(value)
            .bind(histogram)
            .execute(self.pool())
            .await
            .map_err(database_error)?;
        Ok(())
    }
    pub async fn record_quota(
        &self,
        account: &str,
        usage: u32,
        limit: u32,
    ) -> Result<(), StoreError> {
        if account.len() > 128 || limit == 0 {
            return Err(StoreError::InvalidOptions);
        }
        sqlx::query("INSERT INTO remote_quota(account_key,usage,quota_limit) VALUES($1,$2,$3) ON CONFLICT(account_key) DO UPDATE SET usage=EXCLUDED.usage,quota_limit=EXCLUDED.quota_limit,checked_at=now()")
            .bind(account).bind(i64::from(usage)).bind(i64::from(limit)).execute(self.pool()).await.map_err(database_error)?;
        Ok(())
    }
    pub async fn operational_snapshot(
        &self,
        dry_run: bool,
        cap: u32,
    ) -> Result<OperationalSnapshot, StoreError> {
        if !(1..=80).contains(&cap) {
            return Err(StoreError::InvalidOptions);
        }
        // Uma única visão MVCC impede misturar quota, reservas e confirmações da mesma leitura.
        let mut tx = self.pool().begin().await.map_err(database_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let row: SnapshotCounts = sqlx::query_as(r#"
            SELECT paused,
            (SELECT count(*) FROM articles) AS articles,
            (SELECT count(*) FROM jobs WHERE done_at IS NULL AND dead_at IS NULL) AS queue_depth,
            (SELECT count(*) FROM jobs WHERE dead_at IS NOT NULL AND done_at IS NULL) AS dead_jobs,
            (SELECT count(*) FROM publications WHERE dry_run=$1 AND status IN('published','published_unresolved','dry_run') AND COALESCE(published_at,simulated_at)>now()-interval '24 hours') AS published,
            (SELECT count(*) FROM publications WHERE dry_run=$1 AND status IN('creating','container_ready','publishing','uncertain')) AS pending,
            (SELECT usage FROM remote_quota WHERE checked_at>now()-interval '10 minutes' ORDER BY checked_at DESC LIMIT 1) AS remote_usage,
            (SELECT quota_limit FROM remote_quota WHERE checked_at>now()-interval '10 minutes' ORDER BY checked_at DESC LIMIT 1) AS remote_limit,
            COALESCE((SELECT max(detected_at) FROM articles),operational_since)<now()-interval '2 hours' AS no_news,
            COALESCE((SELECT sum(slow)*20>sum(samples) FROM metric_minutes WHERE minute>=now()-interval '5 minutes'),false) AS latency_high,
            (SELECT count(*)=3 AND bool_and(status IN('failed','uncertain')) FROM (SELECT status FROM publications WHERE dry_run=$1 AND status IN('published','published_unresolved','dry_run','failed','uncertain') ORDER BY COALESCE(request_started_at,published_at,simulated_at,created_at) DESC,id DESC LIMIT 3) AS recent) AS publication_failures,
            COALESCE((SELECT max(verified_at) FROM operational_backups),operational_since)<now()-interval '30 hours' AS backup_stale,
            COALESCE((SELECT max(checked_at) FROM operational_restore_checks),operational_since)<now()-interval '35 days' AS restore_stale
            FROM pipeline_control WHERE id=true
        "#).bind(dry_run).fetch_one(&mut *tx).await.map_err(database_error)?;
        let sources = sqlx::query_as("SELECT id, failures=0 AND last_success_at IS NOT NULL AND last_success_at>=monitoring_since AS up, GREATEST(0,extract(epoch FROM now()-GREATEST(COALESCE(last_success_at,monitoring_since),monitoring_since)))::double precision AS silence_seconds FROM sources WHERE enabled AND NOT source_is_blocked(id) ORDER BY id LIMIT 32")
            .fetch_all(&mut *tx).await.map_err(database_error)?;
        let metrics = sqlx::query_as("SELECT name,label,total,samples,buckets FROM metric_totals ORDER BY name,label LIMIT 1024")
            .fetch_all(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        let remote_limit = if dry_run {
            i64::from(cap)
        } else {
            row.remote_limit.unwrap_or(0)
        };
        let used = row
            .published
            .max(if dry_run {
                0
            } else {
                row.remote_usage.unwrap_or(i64::from(cap))
            })
            .saturating_add(row.pending);
        Ok(OperationalSnapshot {
            paused: row.paused,
            articles: row.articles,
            queue_depth: row.queue_depth,
            dead_jobs: row.dead_jobs,
            daily_budget_remaining: i64::from(cap).min(remote_limit).saturating_sub(used).max(0),
            no_news: row.no_news,
            latency_high: row.latency_high,
            publication_failures: row.publication_failures,
            backup_stale: row.backup_stale,
            restore_stale: row.restore_stale,
            sources,
            metrics,
        })
    }
    pub async fn sync_alerts(&self, conditions: &[AlertCondition]) -> Result<(), StoreError> {
        if conditions.len() > 64 || conditions.iter().any(|c| !c.valid()) {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool().begin().await.map_err(database_error)?;
        // Serializa apenas a atualização de condições, nunca a chamada ao Telegram.
        sqlx::query("SELECT pg_advisory_xact_lock(7281934299)")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let codes = conditions.iter().map(|c| c.code).collect::<Vec<_>>();
        let subjects = conditions.iter().map(|c| c.subject).collect::<Vec<_>>();
        sqlx::query("UPDATE operational_alerts SET active=false WHERE code<>'unsupported_text' AND NOT EXISTS(SELECT 1 FROM unnest($1::text[],$2::bigint[]) AS wanted(code,subject) WHERE wanted.code=operational_alerts.code AND wanted.subject=operational_alerts.subject)")
            .bind(codes).bind(subjects).execute(&mut *tx).await.map_err(database_error)?;
        for c in conditions {
            sqlx::query("INSERT INTO operational_alerts(code,subject) VALUES($1,$2) ON CONFLICT(code,subject) DO UPDATE SET active=true,first_seen=CASE WHEN operational_alerts.active THEN operational_alerts.first_seen ELSE now() END,next_attempt=CASE WHEN operational_alerts.active THEN operational_alerts.next_attempt ELSE now() END")
                .bind(c.code).bind(c.subject).execute(&mut *tx).await.map_err(database_error)?;
        }
        sqlx::query("UPDATE operational_alerts SET active=false WHERE code='unsupported_text' AND last_sent IS NOT NULL AND first_seen<now()-interval '24 hours'").execute(&mut *tx).await.map_err(database_error)?;
        sqlx::query("DELETE FROM operational_alerts WHERE NOT active AND first_seen<now()-interval '30 days' AND (lease_until IS NULL OR lease_until<now())").execute(&mut *tx).await.map_err(database_error)?;
        sqlx::query("DELETE FROM metric_minutes WHERE minute<now()-interval '24 hours'")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)
    }
    pub async fn claim_alert(&self) -> Result<Option<AlertLease>, StoreError> {
        sqlx::query_as("WITH candidate AS(SELECT code,subject FROM operational_alerts WHERE active AND next_attempt<=now() AND (lease_until IS NULL OR lease_until<=now()) AND (last_sent IS NULL OR code<>'unsupported_text' AND last_sent<now()-interval '1 hour') ORDER BY next_attempt,first_seen,code,subject FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE operational_alerts a SET lease_token=$1,lease_until=now()+interval '30 seconds' FROM candidate c WHERE a.code=c.code AND a.subject=c.subject RETURNING a.code,a.subject,a.lease_token,a.failures")
            .bind(Uuid::new_v4()).fetch_optional(self.pool()).await.map_err(database_error)
    }
    pub async fn finish_alert(
        &self,
        lease: &AlertLease,
        delivered: bool,
        retry_s: u32,
    ) -> Result<(), StoreError> {
        if !(1..=86400).contains(&retry_s) {
            return Err(StoreError::InvalidOptions);
        }
        let affected=sqlx::query("UPDATE operational_alerts SET last_sent=CASE WHEN $4 THEN now() ELSE last_sent END,failures=CASE WHEN $4 THEN 0 ELSE LEAST(failures+1,32) END,next_attempt=now()+make_interval(secs=>$5::double precision),lease_token=NULL,lease_until=NULL WHERE code=$1 AND subject=$2 AND lease_token=$3 AND lease_until>clock_timestamp()")
            .bind(&lease.code).bind(lease.subject).bind(lease.lease_token).bind(delivered).bind(f64::from(retry_s))
            .execute(self.pool()).await.map_err(database_error)?.rows_affected();
        if affected != 1 {
            return Err(StoreError::LostLease);
        }
        Ok(())
    }
}
