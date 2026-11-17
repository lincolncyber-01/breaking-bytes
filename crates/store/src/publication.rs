//! Publicação persistente: exclusão por conta, fencing de lease e barreira antes do efeito remoto.

use crate::{Job, Store, StoreError, database_error};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{FromRow, Postgres, Transaction};
use stories_scheduler::{Budget, Decision, Policy};

#[derive(Debug, FromRow)]
pub struct Publication {
    pub id: i64,
    pub event_id: i64,
    pub kind: String,
    pub status: String,
    pub container_id: Option<String>,
    pub media_id: Option<String>,
    pub image_key: Option<String>,
    pub dry_run: bool,
    pub account_key: Option<String>,
    pub revision: Option<i64>,
    pub artifact_hash: Option<Vec<u8>>,
    pub request_started_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub cleaned_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
pub struct PublicationArtifact {
    pub artifact_path: String,
    pub content_hash: Vec<u8>,
    pub byte_length: i64,
    pub revision: i64,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PublicationPolicy<'a> {
    pub account: &'a str,
    pub dry_run: bool,
    pub policy: Policy,
    pub remote_usage: u32,
    pub remote_limit: u32,
    pub max_age_hours: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum PublicationStep<'a> {
    UploadPlanned(&'a str),
    Container(&'a str),
    Ready,
    Begin,
    Published(&'a str),
    ConfirmedWithoutId,
    Uncertain,
    Failed,
    Simulated,
}

async fn owned(tx: &mut Transaction<'_, Postgres>, job: &Job) -> Result<(), StoreError> {
    let found:Option<i64>=sqlx::query_scalar("SELECT id FROM jobs WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp() AND done_at IS NULL AND dead_at IS NULL FOR UPDATE")
        .bind(job.id).bind(job.lease_token).fetch_optional(&mut **tx).await.map_err(database_error)?;
    if found.is_none() {
        return Err(StoreError::LostLease);
    }
    Ok(())
}
async fn account_lock(tx: &mut Transaction<'_, Postgres>, account: &str) -> Result<(), StoreError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(account)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    Ok(())
}
async fn defer(
    tx: &mut Transaction<'_, Postgres>,
    job: &Job,
    seconds: i32,
    kind: Option<&str>,
) -> Result<(), StoreError> {
    let n=sqlx::query("UPDATE jobs SET kind=COALESCE($4,kind),attempts=0,run_at=now()+make_interval(secs=>$3::double precision),lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
        .bind(job.id).bind(job.lease_token).bind(seconds).bind(kind).execute(&mut **tx).await.map_err(database_error)?.rows_affected();
    if n != 1 {
        return Err(StoreError::LostLease);
    }
    Ok(())
}
async fn finish(tx: &mut Transaction<'_, Postgres>, job: &Job) -> Result<(), StoreError> {
    let n=sqlx::query("UPDATE jobs SET done_at=now(),lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
        .bind(job.id).bind(job.lease_token).execute(&mut **tx).await.map_err(database_error)?.rows_affected();
    if n != 1 {
        return Err(StoreError::LostLease);
    }
    Ok(())
}
async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    event: i64,
    step: &str,
    detail: Value,
) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO audit_log(event_id,step,detail) VALUES($1,$2,$3)")
        .bind(event)
        .bind(step)
        .bind(detail)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    Ok(())
}
async fn decision(
    tx: &mut Transaction<'_, Postgres>,
    options: PublicationPolicy<'_>,
    exclude: i64,
    score: f64,
    correction: bool,
) -> Result<Decision, StoreError> {
    #[derive(FromRow)]
    struct Counts {
        published: i64,
        pending: i64,
        elapsed: Option<f64>,
    }
    let c:Counts=sqlx::query_as("SELECT count(*) FILTER(WHERE status IN('published','published_unresolved','dry_run') AND published_at>now()-interval '24 hours') AS published,count(*) FILTER(WHERE id<>$3 AND status IN('creating','container_ready','publishing','uncertain')) AS pending,extract(epoch FROM now()-max(COALESCE(request_started_at,published_at)))::double precision AS elapsed FROM publications WHERE account_key=$1 AND dry_run=$2")
        .bind(options.account).bind(options.dry_run).bind(exclude).fetch_one(&mut **tx).await.map_err(database_error)?;
    options
        .policy
        .decide(
            Budget {
                remote_usage: options.remote_usage,
                remote_limit: options.remote_limit,
                local_published: u32::try_from(c.published).unwrap_or(u32::MAX),
                pending: u32::try_from(c.pending).unwrap_or(u32::MAX),
                since_last_seconds: c.elapsed.map(|v| v.max(0.0) as u64),
            },
            score,
            correction,
        )
        .map_err(|_| StoreError::InvalidOptions)
}

impl Store {
    pub async fn publication_artifact(
        &self,
        event: i64,
        revision: i64,
    ) -> Result<PublicationArtifact, StoreError> {
        sqlx::query_as("SELECT artifact_path,content_hash,byte_length,revision,dry_run FROM rendered_stories WHERE event_id=$1 AND revision=$2 ORDER BY created_at DESC LIMIT 1").bind(event).bind(revision).fetch_one(&self.pool).await.map_err(database_error)
    }
    pub async fn publication(&self, id: i64) -> Result<Publication, StoreError> {
        sqlx::query_as("SELECT id,event_id,kind,status,container_id,media_id,image_key,dry_run,account_key,revision,artifact_hash,request_started_at,created_at,cleaned_at FROM publications WHERE id=$1").bind(id).fetch_one(&self.pool).await.map_err(database_error)
    }
    /// Reserva e outbox atômicos. Um JPEG simulado não pode ser promovido implicitamente.
    pub async fn schedule_publication(
        &self,
        job: &Job,
        event: i64,
        revision: i64,
        score: f64,
        signals: Value,
        options: PublicationPolicy<'_>,
    ) -> Result<(), StoreError> {
        if !score.is_finite()
            || !(0.0..=1.0).contains(&score)
            || options.account.is_empty()
            || !(1..=168).contains(&options.max_age_hours)
        {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        owned(&mut tx, job).await?;
        account_lock(&mut tx, options.account).await?;
        #[derive(FromRow)]
        struct Candidate {
            state: String,
            content_revision: i64,
            verified_revision: Option<i64>,
            publication_kind: String,
        }
        let e:Candidate=sqlx::query_as("SELECT state,content_revision,verified_revision,publication_kind FROM events WHERE id=$1 FOR UPDATE").bind(event).fetch_one(&mut *tx).await.map_err(database_error)?;
        let paused: bool =
            sqlx::query_scalar("SELECT paused FROM pipeline_control WHERE id=true FOR SHARE")
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
        if paused {
            defer(&mut tx, job, 5, None).await?;
        } else if e.state != "RENDERED"
            || e.content_revision != revision
            || e.verified_revision != Some(revision)
        {
            finish(&mut tx, job).await?;
        } else {
            let kind = e.publication_kind.as_str();
            let a:PublicationArtifact=sqlx::query_as("SELECT artifact_path,content_hash,byte_length,revision,dry_run FROM rendered_stories WHERE event_id=$1 AND revision=$2 ORDER BY created_at DESC LIMIT 1").bind(event).bind(revision).fetch_one(&mut *tx).await.map_err(database_error)?;
            if a.dry_run != options.dry_run {
                return Err(StoreError::Conflict);
            }
            let fresh: bool = sqlx::query_scalar("SELECT publication_evidence_fresh($1,$2,$3)")
                .bind(event)
                .bind(options.max_age_hours as i32)
                .bind(options.dry_run)
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
            let revision_busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events target JOIN publication_reviews current ON current.id=target.review_id JOIN publication_reviews other ON other.publication_id=current.publication_id JOIN events candidate ON candidate.review_id=other.id JOIN publications p ON p.event_id=candidate.id WHERE target.id=$1 AND p.event_id<>target.id AND p.account_key=$2 AND p.dry_run=$3 AND p.status IN('creating','container_ready','publishing','uncertain'))")
                .bind(event).bind(options.account).bind(options.dry_run).fetch_one(&mut *tx).await.map_err(database_error)?;
            if !fresh {
                sqlx::query("UPDATE events SET state='REJECTED',reason='publication_evidence_expired' WHERE id=$1").bind(event).execute(&mut *tx).await.map_err(database_error)?;
                audit(
                    &mut tx,
                    event,
                    "publication_rejected",
                    json!({"code":"evidence_expired"}),
                )
                .await?;
                finish(&mut tx, job).await?;
            } else if revision_busy {
                // Uma resposta incerta de outra revisão do mesmo Story impede novo efeito remoto.
                audit(
                    &mut tx,
                    event,
                    "revision_publication_wait",
                    json!({"code":"previous_revision_pending"}),
                )
                .await?;
                defer(&mut tx, job, 60, None).await?;
            } else {
                match decision(&mut tx, options, -1, score, kind == "correction").await? {
                    Decision::Ready => {
                        let id:Option<i64>=sqlx::query_scalar("INSERT INTO publications(event_id,kind,seq,status,dry_run,account_key,revision,artifact_hash,editorial_snapshot) VALUES($1,$2,0,'creating',$3,$4,$5,$6,(SELECT jsonb_build_object('draft',draft_json,'assessment',assessment_json) FROM events WHERE id=$1)) ON CONFLICT(event_id,kind,seq) DO NOTHING RETURNING id")
                            .bind(event).bind(kind).bind(options.dry_run).bind(options.account).bind(revision).bind(&a.content_hash).fetch_optional(&mut *tx).await.map_err(database_error)?;
                        if let Some(id) = id {
                            sqlx::query("UPDATE events SET state='QUEUED',score=$2,priority_signals=$3 WHERE id=$1").bind(event).bind(score as f32).bind(&signals).execute(&mut *tx).await.map_err(database_error)?;
                            sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('publish',$1,$2) ON CONFLICT(dedupe_key) DO NOTHING").bind(format!("publish:{id}")).bind(json!({"publication_id":id})).execute(&mut *tx).await.map_err(database_error)?;
                            audit(&mut tx,event,"publication_reserved",json!({"publication_id":id,"dry_run":options.dry_run,"score":score,"signals":signals})).await?;
                        }
                        finish(&mut tx, job).await?;
                    }
                    Decision::Wait(s) => defer(&mut tx, job, s.min(86400) as i32, None).await?,
                    Decision::Capacity | Decision::LowScore => {
                        defer(&mut tx, job, 60, None).await?
                    }
                }
            }
        }
        tx.commit().await.map_err(database_error)
    }
    /// A decisão Begin é a barreira de envio: um novo dono só reconcilia depois dela.
    pub async fn publication_step(
        &self,
        job: &Job,
        id: i64,
        step: PublicationStep<'_>,
        options: PublicationPolicy<'_>,
    ) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        owned(&mut tx, job).await?;
        account_lock(&mut tx, options.account).await?;
        let p:Publication=sqlx::query_as("SELECT id,event_id,kind,status,container_id,media_id,image_key,dry_run,account_key,revision,artifact_hash,request_started_at,created_at,cleaned_at FROM publications WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await.map_err(database_error)?;
        if p.account_key.as_deref() != Some(options.account) || p.dry_run != options.dry_run {
            return Err(StoreError::Conflict);
        }
        if matches!(
            p.status.as_str(),
            "published" | "published_unresolved" | "dry_run" | "failed"
        ) {
            finish(&mut tx, job).await?;
            tx.commit().await.map_err(database_error)?;
            return Ok(false);
        }
        #[derive(FromRow)]
        struct Current {
            state: String,
            content_revision: i64,
            verified_revision: Option<i64>,
            score: Option<f32>,
        }
        let e:Current=sqlx::query_as("SELECT state,content_revision,verified_revision,score FROM events WHERE id=$1 FOR UPDATE").bind(p.event_id).fetch_one(&mut *tx).await.map_err(database_error)?;
        let before = p.request_started_at.is_none();
        if before {
            let paused: bool =
                sqlx::query_scalar("SELECT paused FROM pipeline_control WHERE id=true FOR SHARE")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(database_error)?;
            let fresh: bool = sqlx::query_scalar("SELECT publication_evidence_fresh($1,$2,$3)")
                .bind(p.event_id)
                .bind(options.max_age_hours as i32)
                .bind(options.dry_run)
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
            if paused {
                defer(&mut tx, job, 5, None).await?;
                tx.commit().await.map_err(database_error)?;
                return Ok(false);
            }
            if e.state != "QUEUED"
                || Some(e.content_revision) != p.revision
                || e.verified_revision != p.revision
                || !fresh
            {
                sqlx::query("UPDATE publications SET status='failed' WHERE id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                if e.state == "QUEUED" {
                    if Some(e.content_revision) != p.revision || e.verified_revision != p.revision {
                        // O editorial deve criar o substituto; encerrar o evento aqui perderia a nova evidência.
                        sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('verify',$1,$2) ON CONFLICT(dedupe_key) DO NOTHING")
                            .bind(format!("verify:revision:{}:{}",p.event_id,e.content_revision))
                            .bind(json!({"event_id":p.event_id})).execute(&mut *tx).await.map_err(database_error)?;
                    } else {
                        sqlx::query("UPDATE events SET state='REJECTED',reason='publication_evidence_expired' WHERE id=$1").bind(p.event_id).execute(&mut *tx).await.map_err(database_error)?;
                    }
                }
                audit(
                    &mut tx,
                    p.event_id,
                    "publication_stale",
                    json!({"publication_id":id}),
                )
                .await?;
                finish(&mut tx, job).await?;
                tx.commit().await.map_err(database_error)?;
                return Ok(false);
            }
        }
        let terminal = matches!(
            step,
            PublicationStep::Published(_)
                | PublicationStep::ConfirmedWithoutId
                | PublicationStep::Simulated
                | PublicationStep::Failed
        );
        let (status, code) = match step {
            PublicationStep::UploadPlanned(key)
                if before && p.status == "creating" && !p.dry_run =>
            {
                sqlx::query("UPDATE publications SET image_key=$2 WHERE id=$1")
                    .bind(id)
                    .bind(key)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                ("creating", "upload_planned")
            }
            PublicationStep::Container(container)
                if before && p.status == "creating" && p.image_key.is_some() && !p.dry_run =>
            {
                if container.is_empty() || !container.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(StoreError::InvalidOptions);
                }
                sqlx::query(
                    "UPDATE publications SET container_id=$2 WHERE id=$1 AND container_id IS NULL",
                )
                .bind(id)
                .bind(container)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
                ("creating", "container_created")
            }
            PublicationStep::Ready if before && p.container_id.is_some() && !p.dry_run => {
                ("container_ready", "container_ready")
            }
            PublicationStep::Begin
                if before
                    && p.status == "container_ready"
                    && p.container_id.is_some()
                    && !p.dry_run =>
            {
                match decision(
                    &mut tx,
                    options,
                    id,
                    f64::from(e.score.unwrap_or(0.0)),
                    p.kind == "correction",
                )
                .await?
                {
                    Decision::Ready => {}
                    Decision::Wait(s) => {
                        defer(&mut tx, job, s.min(86400) as i32, None).await?;
                        tx.commit().await.map_err(database_error)?;
                        return Ok(false);
                    }
                    _ => {
                        defer(&mut tx, job, 60, None).await?;
                        tx.commit().await.map_err(database_error)?;
                        return Ok(false);
                    }
                }
                sqlx::query(
                    "UPDATE publications SET request_started_at=clock_timestamp() WHERE id=$1",
                )
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
                sqlx::query("UPDATE events SET state='PUBLISHING' WHERE id=$1")
                    .bind(p.event_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                sqlx::query("UPDATE jobs SET kind='reconcile' WHERE id=$1")
                    .bind(job.id)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                ("publishing", "publish_started")
            }
            PublicationStep::Published(media) if !before && !p.dry_run => {
                if media.is_empty() || !media.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(StoreError::InvalidOptions);
                }
                sqlx::query("UPDATE publications SET media_id=$2,published_at=now(),remote_confirmed_at=now() WHERE id=$1").bind(id).bind(media).execute(&mut *tx).await.map_err(database_error)?;
                ("published", "published")
            }
            PublicationStep::ConfirmedWithoutId if !before && !p.dry_run => {
                sqlx::query("UPDATE publications SET published_at=now(),remote_confirmed_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await.map_err(database_error)?;
                ("published_unresolved", "published_without_media_id")
            }
            PublicationStep::Uncertain if !before && !p.dry_run => ("uncertain", "uncertain"),
            PublicationStep::Simulated if before && p.dry_run => {
                sqlx::query("UPDATE publications SET published_at=now() WHERE id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                ("dry_run", "dry_run_completed")
            }
            PublicationStep::Failed if before => ("failed", "failed_before_publish"),
            _ => return Err(StoreError::Conflict),
        };
        sqlx::query("UPDATE publications SET status=$2,reconciled_at=CASE WHEN $2='uncertain' THEN now() ELSE reconciled_at END WHERE id=$1").bind(id).bind(status).execute(&mut *tx).await.map_err(database_error)?;
        if matches!(status, "published" | "published_unresolved") {
            sqlx::query("UPDATE events SET state='PUBLISHED' WHERE id=$1 AND state='PUBLISHING'")
                .bind(p.event_id)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        } else if status == "failed" {
            sqlx::query("UPDATE events SET state='FAILED',reason='publication_failed' WHERE id=$1 AND state='QUEUED'").bind(p.event_id).execute(&mut *tx).await.map_err(database_error)?;
        }
        if matches!(status, "published" | "published_unresolved" | "dry_run") {
            sqlx::query("UPDATE publication_watches SET superseded_by=$2 WHERE publication_id=(SELECT r.publication_id FROM events e JOIN publication_reviews r ON r.id=e.review_id WHERE e.id=$1) AND superseded_by IS NULL").bind(p.event_id).bind(p.id).execute(&mut *tx).await.map_err(database_error)?;
        }
        audit(
            &mut tx,
            p.event_id,
            code,
            json!({"publication_id":id,"dry_run":p.dry_run}),
        )
        .await?;
        if terminal {
            finish(&mut tx, job).await?;
        } else if matches!(step, PublicationStep::Uncertain) {
            defer(&mut tx, job, 60, Some("reconcile")).await?;
        }
        tx.commit().await.map_err(database_error)?;
        Ok(true)
    }
    /// Recupera intenções mesmo se uma falha de transporte tiver esgotado o job original.
    pub async fn recover_publications(
        &self,
        account: &str,
        dry_run: bool,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE jobs j SET attempts=0,dead_at=NULL,run_at=now(),kind=CASE WHEN p.request_started_at IS NULL THEN 'publish' ELSE 'reconcile' END FROM publications p WHERE j.dedupe_key='publish:'||p.id AND p.account_key=$1 AND p.dry_run=$2 AND p.status IN('creating','container_ready','publishing','uncertain') AND j.done_at IS NULL AND (j.lease_until IS NULL OR j.lease_until<=now()) AND (j.dead_at IS NOT NULL OR j.attempts>=5)").bind(account).bind(dry_run).execute(&self.pool).await.map_err(database_error)?;
        Ok(())
    }
    pub async fn defer_publication(&self, job: &Job, seconds: i32) -> Result<(), StoreError> {
        if !(1..=86400).contains(&seconds) {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        owned(&mut tx, job).await?;
        defer(&mut tx, job, seconds, None).await?;
        tx.commit().await.map_err(database_error)
    }
    pub async fn cleanup_candidates(
        &self,
        account: &str,
        ttl_minutes: i32,
    ) -> Result<Vec<Publication>, StoreError> {
        sqlx::query_as("SELECT id,event_id,kind,status,container_id,media_id,image_key,dry_run,account_key,revision,artifact_hash,request_started_at,created_at,cleaned_at FROM publications WHERE account_key=$1 AND NOT dry_run AND cleaned_at IS NULL AND image_key IS NOT NULL AND status IN('published','published_unresolved','failed') AND GREATEST(created_at,COALESCE(published_at,created_at))<now()-make_interval(mins=>$2) ORDER BY id LIMIT 20").bind(account).bind(ttl_minutes).fetch_all(&self.pool).await.map_err(database_error)
    }
    pub async fn publication_cleaned(&self, id: i64) -> Result<(), StoreError> {
        sqlx::query("UPDATE publications SET cleaned_at=now() WHERE id=$1 AND status IN('published','published_unresolved','failed')").bind(id).execute(&self.pool).await.map_err(database_error)?;
        Ok(())
    }
}
