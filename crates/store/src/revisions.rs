//! Reconsulta durável: baseline da publicação, observações e candidato editorial em transações curtas.

use crate::{Job, Store, StoreError, database_error};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{FromRow, Postgres, Transaction};
use stories_core::CapturedRevision;
use stories_editorial::{
    Draft,
    revisions::{ChangeDecision, ChangeReview, RevisionContext},
};
use stories_verify::{
    Assessment, Evidence, Level,
    revisions::{SourceRevision, fingerprint},
};

#[derive(Clone, Copy)]
pub struct WatchPolicy {
    pub window_hours: u32,
    pub high_impact_hours: u32,
    pub high_impact_score: f64,
    pub interval_s: u32,
    pub max_sources: usize,
}
pub struct WatchSource {
    pub baseline: Evidence,
    pub active: bool,
}
pub struct WatchContext {
    pub publication_id: i64,
    pub event_id: i64,
    pub dry_run: bool,
    pub version: i64,
    pub active: bool,
    pub published: Draft,
    pub sources: Vec<WatchSource>,
}
pub struct SourceObservation {
    pub article_id: i64,
    pub source_id: i32,
    pub captured: Option<CapturedRevision>,
    pub error: Option<&'static str>,
}
#[derive(Debug)]
pub struct ReviewRecord {
    pub id: i64,
    pub status: String,
    pub context: RevisionContext,
    pub decision: Option<ChangeReview>,
}

async fn owned(tx: &mut Transaction<'_, Postgres>, job: &Job) -> Result<(), StoreError> {
    let found:Option<i64>=sqlx::query_scalar("SELECT id FROM jobs WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp() AND done_at IS NULL AND dead_at IS NULL FOR UPDATE").bind(job.id).bind(job.lease_token).fetch_optional(&mut **tx).await.map_err(database_error)?;
    if found.is_none() {
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
async fn identity(tx: &mut Transaction<'_, Postgres>, e: &Evidence) -> Result<bool, StoreError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sources WHERE id=$1 AND enabled AND NOT source_is_blocked(id) AND name=$2 AND url=$3 AND kind=$4 AND layer::text=$5 AND weight=$6 AND origin_group IS NOT DISTINCT FROM $7)")
        .bind(e.source_id).bind(&e.source_name).bind(&e.source_url).bind(&e.kind).bind(&e.layer).bind(e.weight).bind(&e.origin_group).fetch_one(&mut **tx).await.map_err(database_error)
}
async fn source_error(
    tx: &mut Transaction<'_, Postgres>,
    publication: i64,
    article: i64,
    source: i32,
    code: &str,
) -> Result<(), StoreError> {
    if !matches!(
        code,
        "http" | "parse" | "limits" | "kind" | "worker" | "unavailable" | "identity_changed"
    ) {
        return Err(StoreError::InvalidOptions);
    }
    sqlx::query("UPDATE publication_source_snapshots SET attempted_at=now(),failures=LEAST(failures+1,32),last_error_code=$4 WHERE publication_id=$1 AND article_id=$2 AND source_id=$3")
        .bind(publication).bind(article).bind(source).bind(code).execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}
#[derive(FromRow)]
struct Snapshot {
    baseline: Value,
    current_snapshot: Option<Value>,
    content_hash: Option<String>,
    notice: Option<String>,
    fetched_at: Option<DateTime<Utc>>,
    last_error_code: Option<String>,
}

impl Store {
    pub async fn defer_revision_watch(&self, job: &Job, delay: u32) -> Result<(), StoreError> {
        if job.kind != "revisit" || !(60..=3600).contains(&delay) {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        owned(&mut tx, job).await?;
        let n=sqlx::query("UPDATE jobs SET run_at=now()+make_interval(secs=>$3::double precision),attempts=0,lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
            .bind(job.id).bind(job.lease_token).bind(delay as i32).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        if n != 1 {
            return Err(StoreError::LostLease);
        }
        tx.commit().await.map_err(database_error)
    }
    /// Retoma inclusive publicações cujo processo caiu antes de criar o acompanhamento.
    pub async fn enroll_watches(
        &self,
        policy: WatchPolicy,
        dry_run: bool,
    ) -> Result<u64, StoreError> {
        if !(1..=72).contains(&policy.window_hours)
            || !(policy.window_hours..=168).contains(&policy.high_impact_hours)
            || !(60..=3600).contains(&policy.interval_s)
            || !(1..=4).contains(&policy.max_sources)
            || !policy.high_impact_score.is_finite()
            || !(0.0..=1.0).contains(&policy.high_impact_score)
        {
            return Err(StoreError::InvalidOptions);
        }
        #[derive(FromRow)]
        struct Candidate {
            id: i64,
            event_id: i64,
            published_at: DateTime<Utc>,
            editorial_snapshot: Value,
            score: Option<f32>,
        }
        let candidates:Vec<Candidate>=sqlx::query_as("SELECT p.id,p.event_id,COALESCE(p.published_at,p.simulated_at) AS published_at,p.editorial_snapshot,e.score FROM publications p JOIN events e ON e.id=p.event_id WHERE p.dry_run=$1 AND p.status IN('published','published_unresolved','dry_run') AND COALESCE(p.published_at,p.simulated_at)>now()-make_interval(hours=>$2) AND jsonb_typeof(p.editorial_snapshot->'draft')='object' AND jsonb_typeof(p.editorial_snapshot->'assessment')='object' AND NOT EXISTS(SELECT 1 FROM publication_watches WHERE publication_id=p.id) ORDER BY p.id LIMIT 32")
            .bind(dry_run).bind(policy.high_impact_hours as i32).fetch_all(&self.pool).await.map_err(database_error)?;
        let mut inserted = 0;
        for c in candidates {
            let draft: Draft = serde_json::from_value(
                c.editorial_snapshot
                    .get("draft")
                    .cloned()
                    .ok_or(StoreError::InvalidJob)?,
            )
            .map_err(|_| StoreError::InvalidJob)?;
            let assessment: Assessment = serde_json::from_value(
                c.editorial_snapshot
                    .get("assessment")
                    .cloned()
                    .ok_or(StoreError::InvalidJob)?,
            )
            .map_err(|_| StoreError::InvalidJob)?;
            if assessment.level == Level::Observation || assessment.sources.is_empty() {
                continue;
            }
            let hours = if f64::from(c.score.unwrap_or(0.0)) >= policy.high_impact_score {
                policy.high_impact_hours
            } else {
                policy.window_hours
            };
            let expires = c.published_at + chrono::Duration::hours(i64::from(hours));
            if expires <= Utc::now() {
                continue;
            }
            let mut tx = self.pool.begin().await.map_err(database_error)?;
            let n=sqlx::query("INSERT INTO publication_watches(publication_id,event_id,dry_run,baseline_draft,expires_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                .bind(c.id).bind(c.event_id).bind(dry_run).bind(serde_json::to_value(draft).map_err(|_|StoreError::InvalidJob)?).bind(expires).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
            if n == 1 {
                for e in assessment.sources.into_iter().take(policy.max_sources) {
                    sqlx::query("INSERT INTO publication_source_snapshots(publication_id,article_id,source_id,baseline) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
                        .bind(c.id).bind(e.article_id).bind(e.source_id).bind(serde_json::to_value(e).map_err(|_|StoreError::InvalidJob)?).execute(&mut *tx).await.map_err(database_error)?;
                }
                sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload,run_at) VALUES('revisit',$1,$2,now()+make_interval(secs=>$3::double precision)) ON CONFLICT DO NOTHING")
                    .bind(format!("revisit:{}",c.id)).bind(json!({"publication_id":c.id,"dry_run":dry_run})).bind(policy.interval_s as i32).execute(&mut *tx).await.map_err(database_error)?;
                audit(
                    &mut tx,
                    c.event_id,
                    "revision_watch_started",
                    json!({"publication_id":c.id,"dry_run":dry_run,"expires_at":expires}),
                )
                .await?;
                inserted += 1;
            }
            tx.commit().await.map_err(database_error)?;
        }
        Ok(inserted)
    }
    pub async fn watch_context(&self, publication: i64) -> Result<WatchContext, StoreError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let (event,dry_run,version,active,published):(i64,bool,i64,bool,Value)=sqlx::query_as("SELECT event_id,dry_run,version,superseded_by IS NULL AND expires_at>now(),baseline_draft FROM publication_watches WHERE publication_id=$1").bind(publication).fetch_one(&mut *tx).await.map_err(database_error)?;
        let rows:Vec<Value>=sqlx::query_scalar("SELECT baseline FROM publication_source_snapshots WHERE publication_id=$1 ORDER BY source_id,article_id LIMIT 4").bind(publication).fetch_all(&mut *tx).await.map_err(database_error)?;
        let mut sources = Vec::new();
        for row in rows {
            let baseline: Evidence =
                serde_json::from_value(row).map_err(|_| StoreError::InvalidJob)?;
            let active = identity(&mut tx, &baseline).await?;
            sources.push(WatchSource { baseline, active });
        }
        tx.commit().await.map_err(database_error)?;
        Ok(WatchContext {
            publication_id: publication,
            event_id: event,
            dry_run,
            version,
            active,
            published: serde_json::from_value(published).map_err(|_| StoreError::InvalidJob)?,
            sources,
        })
    }
    pub async fn observe_revision(
        &self,
        job: &Job,
        context: &WatchContext,
        observations: Vec<SourceObservation>,
        interval_s: u32,
        age: u32,
    ) -> Result<(), StoreError> {
        if job.kind != "revisit"
            || job.payload.get("publication_id").and_then(Value::as_i64)
                != Some(context.publication_id)
            || observations.len() != context.sources.len()
            || observations.len() > 4
            || !(60..=3600).contains(&interval_s)
            || !(1..=168).contains(&age)
        {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        owned(&mut tx, job).await?;
        let (version,active,previous):(i64,bool,Option<Vec<u8>>)=sqlx::query_as("SELECT version,superseded_by IS NULL AND expires_at>now(),fingerprint FROM publication_watches WHERE publication_id=$1 FOR UPDATE").bind(context.publication_id).fetch_one(&mut *tx).await.map_err(database_error)?;
        if !active {
            finish(&mut tx, job).await?;
            tx.commit().await.map_err(database_error)?;
            return Ok(());
        }
        if version != context.version {
            return Err(StoreError::Conflict);
        }
        for o in observations {
            let Some(source) = context.sources.iter().find(|s| {
                s.baseline.article_id == o.article_id && s.baseline.source_id == o.source_id
            }) else {
                return Err(StoreError::InvalidJob);
            };
            let stable = identity(&mut tx, &source.baseline).await?;
            let code = if !stable {
                Some("identity_changed")
            } else {
                o.error
            };
            if let Some(captured) = o.captured.filter(|_| stable && code.is_none()) {
                let mut after = source.baseline.clone();
                after.title = captured.article.title;
                after.excerpt = captured.article.excerpt;
                after.published_at = captured.article.published_at;
                after.date_confirmed = after.published_at.is_some_and(|t| t <= Utc::now());
                let hash = fingerprint(&after, captured.notice.as_deref())
                    .map_err(|_| StoreError::InvalidJob)?;
                let revision = SourceRevision {
                    before: source.baseline.clone(),
                    after: after.clone(),
                    notice: captured.notice.clone(),
                    observed_at: Utc::now(),
                    after_hash: blake3::Hash::from_bytes(hash).to_hex().to_string(),
                };
                if captured.article.url != after.article_url || !revision.eligible(Utc::now(), age)
                {
                    source_error(
                        &mut tx,
                        context.publication_id,
                        o.article_id,
                        o.source_id,
                        "parse",
                    )
                    .await?;
                    continue;
                }
                sqlx::query("UPDATE publication_source_snapshots SET current_snapshot=$4,content_hash=$5,notice=$6,fetched_at=now(),attempted_at=now(),failures=0,last_error_code=NULL WHERE publication_id=$1 AND article_id=$2 AND source_id=$3")
                    .bind(context.publication_id).bind(o.article_id).bind(o.source_id).bind(serde_json::to_value(after).map_err(|_|StoreError::InvalidJob)?).bind(revision.after_hash).bind(captured.notice).execute(&mut *tx).await.map_err(database_error)?;
            } else {
                source_error(
                    &mut tx,
                    context.publication_id,
                    o.article_id,
                    o.source_id,
                    code.unwrap_or("unavailable"),
                )
                .await?;
            }
        }
        let rows:Vec<Snapshot>=sqlx::query_as("SELECT article_id,source_id,baseline,current_snapshot,content_hash,notice,fetched_at,last_error_code FROM publication_source_snapshots WHERE publication_id=$1 ORDER BY article_id,source_id").bind(context.publication_id).fetch_all(&mut *tx).await.map_err(database_error)?;
        let mut changes = Vec::new();
        for row in rows {
            if row.last_error_code.is_some() {
                continue;
            }
            if let (Some(after), Some(hash), Some(observed_at)) =
                (row.current_snapshot, row.content_hash, row.fetched_at)
            {
                let revision = SourceRevision {
                    before: serde_json::from_value(row.baseline)
                        .map_err(|_| StoreError::InvalidJob)?,
                    after: serde_json::from_value(after).map_err(|_| StoreError::InvalidJob)?,
                    notice: row.notice,
                    observed_at,
                    after_hash: hash,
                };
                if revision.changed() && revision.eligible(Utc::now(), age) {
                    changes.push(revision);
                }
            }
        }
        let signature = serde_json::to_vec(
            &changes
                .iter()
                .map(|c| (c.after.article_id, c.after.source_id, &c.after_hash))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| StoreError::InvalidJob)?;
        let digest = *blake3::hash(&signature).as_bytes();
        if previous.as_deref() != Some(digest.as_slice()) {
            let next = version + 1;
            sqlx::query(
                "UPDATE publication_watches SET version=$2,fingerprint=$3 WHERE publication_id=$1",
            )
            .bind(context.publication_id)
            .bind(next)
            .bind(digest.as_slice())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            if !changes.is_empty() {
                let assessment = stories_verify::revisions::assess(&changes, Utc::now(), age);
                if assessment.level != Level::Observation {
                    changes.retain(|c| {
                        assessment.sources.iter().any(|e| {
                            e.article_id == c.after.article_id && e.source_id == c.after.source_id
                        })
                    });
                }
                let review = RevisionContext {
                    publication_id: context.publication_id,
                    version: next,
                    max_age_hours: age,
                    dry_run: context.dry_run,
                    published: context.published.clone(),
                    changes,
                };
                let id:i64=sqlx::query_scalar("INSERT INTO publication_reviews(publication_id,version,context) VALUES($1,$2,$3) RETURNING id").bind(context.publication_id).bind(next).bind(serde_json::to_value(review).map_err(|_|StoreError::InvalidJob)?).fetch_one(&mut *tx).await.map_err(database_error)?;
                sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('review_revision',$1,$2) ON CONFLICT DO NOTHING").bind(format!("review_revision:{id}")).bind(json!({"review_id":id,"dry_run":context.dry_run})).execute(&mut *tx).await.map_err(database_error)?;
                audit(&mut tx,context.event_id,"source_revision_detected",json!({"publication_id":context.publication_id,"review_id":id,"version":next,"dry_run":context.dry_run})).await?;
            }
        }
        let (failed,total,failures):(i64,i64,i32)=sqlx::query_as("SELECT count(*) FILTER(WHERE last_error_code IS NOT NULL),count(*),COALESCE(min(failures),0) FROM publication_source_snapshots WHERE publication_id=$1")
            .bind(context.publication_id).fetch_one(&mut *tx).await.map_err(database_error)?;
        let multiplier = if failed == total && total > 0 {
            1u32 << failures.clamp(0, 4)
        } else {
            1
        };
        let jitter = if multiplier > 1 {
            context.publication_id.unsigned_abs() % 31
        } else {
            0
        };
        let delay = (u64::from(interval_s) * u64::from(multiplier) + jitter).min(3600) as i32;
        let n=sqlx::query("UPDATE jobs SET run_at=now()+make_interval(secs=>$3::double precision),attempts=0,lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
            .bind(job.id).bind(job.lease_token).bind(delay).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        if n != 1 {
            return Err(StoreError::LostLease);
        }
        tx.commit().await.map_err(database_error)
    }
    pub async fn review_record(&self, id: i64) -> Result<ReviewRecord, StoreError> {
        let (status, context, decision): (String, Value, Option<Value>) =
            sqlx::query_as("SELECT status,context,decision FROM publication_reviews WHERE id=$1")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(database_error)?;
        let mut context: RevisionContext =
            serde_json::from_value(context).map_err(|_| StoreError::InvalidJob)?;
        for c in &mut context.changes {
            let fetched:Option<DateTime<Utc>>=sqlx::query_scalar("SELECT fetched_at FROM publication_source_snapshots WHERE publication_id=$1 AND article_id=$2 AND source_id=$3 AND content_hash=$4 AND last_error_code IS NULL")
                .bind(context.publication_id).bind(c.after.article_id).bind(c.after.source_id).bind(&c.after_hash).fetch_optional(&self.pool).await.map_err(database_error)?.flatten();
            if let Some(t) = fetched {
                c.observed_at = t;
            } else {
                c.observed_at = DateTime::<Utc>::UNIX_EPOCH;
            }
        }
        Ok(ReviewRecord {
            id,
            status,
            context,
            decision: decision
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| StoreError::InvalidJob)?,
        })
    }
    pub async fn apply_revision_review(
        &self,
        job: &Job,
        record: &ReviewRecord,
        review: ChangeReview,
        age: u32,
    ) -> Result<Option<i64>, StoreError> {
        if job.kind != "review_revision"
            || job.payload.get("review_id").and_then(Value::as_i64) != Some(record.id)
        {
            return Err(StoreError::InvalidJob);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        owned(&mut tx, job).await?;
        let (status,version,event,active,stored):(String,i64,i64,bool,Value)=sqlx::query_as("SELECT r.status,w.version,w.event_id,w.superseded_by IS NULL AND w.expires_at>now(),r.context FROM publication_reviews r JOIN publication_watches w ON w.publication_id=r.publication_id WHERE r.id=$1 FOR UPDATE OF r,w").bind(record.id).fetch_one(&mut *tx).await.map_err(database_error)?;
        if status != "pending" || version != record.context.version || !active {
            if status == "pending" {
                sqlx::query("UPDATE publication_reviews SET status='superseded',decided_at=now() WHERE id=$1").bind(record.id).execute(&mut *tx).await.map_err(database_error)?;
            }
            finish(&mut tx, job).await?;
            tx.commit().await.map_err(database_error)?;
            return Ok(None);
        }
        let mut current: RevisionContext =
            serde_json::from_value(stored).map_err(|_| StoreError::InvalidJob)?;
        let mut fresh = true;
        for change in &mut current.changes {
            let fetched:Option<DateTime<Utc>>=sqlx::query_scalar("SELECT fetched_at FROM publication_source_snapshots WHERE publication_id=$1 AND article_id=$2 AND source_id=$3 AND content_hash=$4 AND last_error_code IS NULL FOR SHARE")
                .bind(current.publication_id).bind(change.after.article_id).bind(change.after.source_id).bind(&change.after_hash).fetch_optional(&mut *tx).await.map_err(database_error)?.flatten();
            change.observed_at = fetched.unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
            fresh &= identity(&mut tx, &change.before).await?
                && change.eligible(Utc::now(), age.min(current.max_age_hours));
        }
        if review.decision.publication_kind().is_some() && !fresh {
            sqlx::query(
                "UPDATE publication_reviews SET status='superseded',decided_at=now() WHERE id=$1",
            )
            .bind(record.id)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            audit(
                &mut tx,
                event,
                "revision_review_obsolete",
                json!({"review_id":record.id}),
            )
            .await?;
            finish(&mut tx, job).await?;
            tx.commit().await.map_err(database_error)?;
            return Ok(None);
        }
        let assessment = current.assessment(Utc::now(), age);
        stories_editorial::revisions::validate(&review, &current, &assessment, Utc::now(), age)
            .map_err(|_| StoreError::InvalidOptions)?;
        let mut candidate = None;
        let outcome = if let Some(kind) = review.decision.publication_kind() {
            let child:i64=sqlx::query_scalar("INSERT INTO events(parent_event_id,primary_article_id,publication_kind,review_id,entities) SELECT id,primary_article_id,$2,$3,entities FROM events WHERE id=$1 RETURNING id")
                .bind(event).bind(kind).bind(record.id).fetch_one(&mut *tx).await.map_err(database_error)?;
            sqlx::query("INSERT INTO event_articles(event_id,article_id) SELECT $2,article_id FROM event_articles WHERE event_id=$1").bind(event).bind(child).execute(&mut *tx).await.map_err(database_error)?;
            sqlx::query("UPDATE events SET state='CLUSTERED' WHERE id=$1")
                .bind(child)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('verify',$1,$2)")
                .bind(format!("verify:review:{child}"))
                .bind(json!({"event_id":child}))
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            if kind == "correction" {
                sqlx::query("UPDATE events SET state='NEEDS_CORRECTION',reason='source_correction_confirmed' WHERE id=$1 AND state='PUBLISHED'").bind(event).execute(&mut *tx).await.map_err(database_error)?;
            }
            candidate = Some(child);
            "applied"
        } else if review.decision == ChangeDecision::Unchanged {
            "ignored"
        } else {
            "uncertain"
        };
        sqlx::query("UPDATE publication_reviews SET status=$2,decision=$3,candidate_event_id=$4,decided_at=now() WHERE id=$1").bind(record.id).bind(outcome).bind(serde_json::to_value(&review).map_err(|_|StoreError::InvalidJob)?).bind(candidate).execute(&mut *tx).await.map_err(database_error)?;
        audit(&mut tx,event,"revision_reviewed",json!({"review_id":record.id,"decision":review.decision,"candidate_event_id":candidate,"dry_run":record.context.dry_run})).await?;
        finish(&mut tx, job).await?;
        tx.commit().await.map_err(database_error)?;
        Ok(candidate)
    }
    pub async fn revision_context_for_event(
        &self,
        event: i64,
    ) -> Result<Option<ReviewRecord>, StoreError> {
        let id: Option<i64> = sqlx::query_scalar("SELECT review_id FROM events WHERE id=$1")
            .bind(event)
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)?;
        match id {
            Some(id) => self.review_record(id).await.map(Some),
            None => Ok(None),
        }
    }
    pub async fn revision_fresh(
        &self,
        event: i64,
        age: u32,
        dry_run: bool,
    ) -> Result<bool, StoreError> {
        sqlx::query_scalar("SELECT revision_evidence_fresh($1,$2,$3)")
            .bind(event)
            .bind(age as i32)
            .bind(dry_run)
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)
    }
}
