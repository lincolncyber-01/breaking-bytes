//! Fencing de lease, versão editorial e revisão das fontes em toda confirmação.

use crate::{Job, Store, StoreError, database_error};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{FromRow, Postgres, Transaction};
use stories_core::EventState;
use stories_editorial::{Draft, FactReport};
use stories_verify::{Assessment, Evidence, Level};

#[derive(Debug, FromRow)]
struct Row {
    id: i64,
    state: String,
    reason: Option<String>,
    content_revision: i64,
    editorial_version: i64,
    verified_revision: Option<i64>,
    editorial_attempts: i32,
    assessment_json: Option<Value>,
    draft_json: Option<Value>,
    created_at: DateTime<Utc>,
    publication_kind: String,
    review_id: Option<i64>,
}

#[derive(Debug)]
pub struct EditorialContext {
    pub event_id: i64,
    pub state: EventState,
    pub reason: Option<String>,
    pub revision: i64,
    pub version: i64,
    pub verified_revision: Option<i64>,
    pub rewrites: i32,
    pub assessment: Option<Assessment>,
    pub draft: Option<Draft>,
    pub created_at: DateTime<Utc>,
    pub kind: &'static str,
    pub evidence: Vec<Evidence>,
    pub review: Option<crate::ReviewRecord>,
}

pub enum EditorialStep {
    Observe {
        reason: String,
        expired: bool,
    },
    Verified(Assessment),
    Drafted(Draft),
    Checked(FactReport),
    CheckedRevision {
        report: FactReport,
        review: stories_editorial::revisions::ChangeReview,
    },
    Rewrite {
        code: String,
        limit: u8,
    },
    Failed {
        code: String,
    },
    Rejected {
        code: String,
    },
    Skip,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorialOutcome {
    Progress,
    Finished,
    Restarted,
    Deferred,
}

impl Store {
    pub async fn synchronize_sources(
        &self,
        sources: &[(String, Option<String>, bool)],
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let names = sources
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect::<Vec<_>>();
        sqlx::query("UPDATE sources SET enabled=false WHERE NOT(name=ANY($1)) AND enabled")
            .bind(names)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        for (name, origin, enabled) in sources {
            sqlx::query("UPDATE sources SET origin_group=$2,enabled=$3 WHERE name=$1")
                .bind(name)
                .bind(origin)
                .bind(enabled)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        }
        tx.commit().await.map_err(database_error)
    }

    pub async fn editorial_context(&self, event: i64) -> Result<EditorialContext, StoreError> {
        let row:Row=sqlx::query_as("SELECT id,state,reason,content_revision,editorial_version,verified_revision,editorial_attempts,assessment_json,draft_json,created_at,publication_kind,review_id FROM events WHERE id=$1")
            .bind(event).fetch_one(&self.pool).await.map_err(database_error)?;
        #[derive(FromRow)]
        struct SourceRow {
            article_id: i64,
            source_id: i32,
            source_name: String,
            source_url: String,
            article_url: String,
            layer: String,
            kind: String,
            weight: i16,
            origin_group: Option<String>,
            published_at: Option<DateTime<Utc>>,
            title: String,
            body_excerpt: Option<String>,
        }
        let evidence:Vec<SourceRow>=sqlx::query_as("SELECT a.id article_id,s.id source_id,s.name source_name,s.url source_url,a.url_canonical article_url,s.layer::text layer,s.kind,s.weight,s.origin_group,a.published_at,a.title,a.body_excerpt FROM event_articles ea JOIN articles a ON a.id=ea.article_id JOIN article_sightings sight ON sight.article_id=a.id JOIN sources s ON s.id=sight.source_id WHERE ea.event_id=$1 AND s.enabled AND NOT source_is_blocked(s.id) ORDER BY s.weight DESC,a.detected_at,a.id,s.id LIMIT 32")
            .bind(event).fetch_all(&self.pool).await.map_err(database_error)?;
        Ok(EditorialContext {
            event_id: row.id,
            state: EventState::parse(&row.state)?,
            reason: row.reason,
            revision: row.content_revision,
            version: row.editorial_version,
            verified_revision: row.verified_revision,
            rewrites: row.editorial_attempts,
            assessment: row
                .assessment_json
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| StoreError::InvalidJob)?,
            draft: row
                .draft_json
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| StoreError::InvalidJob)?,
            created_at: row.created_at,
            kind: match row.publication_kind.as_str() {
                "new" => "new",
                "update" => "update",
                "correction" => "correction",
                _ => return Err(StoreError::InvalidJob),
            },
            review: match row.review_id {
                Some(id) => Some(self.review_record(id).await?),
                None => None,
            },
            evidence: evidence
                .into_iter()
                .map(|s| Evidence {
                    article_id: s.article_id,
                    source_id: s.source_id,
                    source_name: s.source_name,
                    source_url: s.source_url,
                    article_url: s.article_url,
                    layer: s.layer,
                    kind: s.kind,
                    weight: s.weight,
                    origin_group: s.origin_group,
                    published_at: s.published_at,
                    date_confirmed: false,
                    title: s.title,
                    excerpt: s.body_excerpt.unwrap_or_default(),
                })
                .collect(),
        })
    }

    pub async fn editorial_commit(
        &self,
        job: &Job,
        context: &EditorialContext,
        step: EditorialStep,
    ) -> Result<EditorialOutcome, StoreError> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let owned:Option<i64>=sqlx::query_scalar("SELECT id FROM jobs WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp() AND done_at IS NULL AND dead_at IS NULL FOR UPDATE")
            .bind(job.id).bind(job.lease_token).fetch_optional(&mut *tx).await.map_err(database_error)?;
        if owned.is_none() {
            return Err(StoreError::LostLease);
        }
        let row:Row=sqlx::query_as("SELECT id,state,reason,content_revision,editorial_version,verified_revision,editorial_attempts,assessment_json,draft_json,created_at,publication_kind,review_id FROM events WHERE id=$1 FOR UPDATE")
            .bind(context.event_id).fetch_one(&mut *tx).await.map_err(database_error)?;
        let state = EventState::parse(&row.state)?;
        if state.is_terminal() || matches!(state, EventState::Publishing | EventState::Published) {
            finish(&mut tx, job).await?;
            tx.commit().await.map_err(database_error)?;
            return Ok(EditorialOutcome::Finished);
        }
        let blocked:bool=sqlx::query_scalar("SELECT event_is_blocked($1)").bind(row.id).fetch_one(&mut *tx).await.map_err(database_error)?;
        if blocked {terminal(&mut tx,row.id,"REJECTED","policy_blocked").await?;finish(&mut tx,job).await?;tx.commit().await.map_err(database_error)?;return Ok(EditorialOutcome::Finished);}
        if let Some(review) = &context.review {
            let fresh: bool = sqlx::query_scalar("SELECT revision_evidence_fresh($1,$2,$3)")
                .bind(row.id)
                .bind(review.context.max_age_hours as i32)
                .bind(review.context.dry_run)
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
            if !fresh {
                terminal(&mut tx, row.id, "SUPERSEDED", "source_revision_obsolete").await?;
                finish(&mut tx, job).await?;
                tx.commit().await.map_err(database_error)?;
                return Ok(EditorialOutcome::Finished);
            }
        }
        let stale = row.content_revision != context.revision
            || row
                .verified_revision
                .is_some_and(|revision| revision != row.content_revision);
        if stale {
            if state == EventState::Clustered {
                enqueue(&mut tx, row.id, row.content_revision).await?;
            } else {
                sqlx::query("UPDATE events SET state='SUPERSEDED',reason='evidence_changed',editorial_version=editorial_version+1 WHERE id=$1")
                    .bind(row.id).execute(&mut *tx).await.map_err(database_error)?;
                let child:i64=sqlx::query_scalar("INSERT INTO events(parent_event_id,primary_article_id,entities,publication_kind,review_id) SELECT parent_event_id,primary_article_id,entities,publication_kind,review_id FROM events WHERE id=$1 RETURNING id")
                    .bind(row.id).fetch_one(&mut *tx).await.map_err(database_error)?;
                sqlx::query("INSERT INTO event_articles(event_id,article_id) SELECT $2,article_id FROM event_articles WHERE event_id=$1")
                    .bind(row.id).bind(child).execute(&mut *tx).await.map_err(database_error)?;
                sqlx::query("UPDATE events SET state='CLUSTERED' WHERE id=$1")
                    .bind(child)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                enqueue(&mut tx, child, 0).await?;
                audit(
                    &mut tx,
                    row.id,
                    "evidence_superseded",
                    json!({"replacement":child}),
                )
                .await?;
            }
            finish(&mut tx, job).await?;
            tx.commit().await.map_err(database_error)?;
            return Ok(EditorialOutcome::Restarted);
        }
        if row.editorial_version != context.version || state != context.state {
            finish(&mut tx, job).await?;
            tx.commit().await.map_err(database_error)?;
            return Ok(EditorialOutcome::Finished);
        }
        let mut outcome = EditorialOutcome::Progress;
        match step {
            EditorialStep::Observe { reason, expired } if state == EventState::Clustered => {
                if expired {
                    terminal(&mut tx, row.id, "REJECTED", "unconfirmed_expired").await?;
                    finish(&mut tx, job).await?;
                    outcome = EditorialOutcome::Finished;
                } else {
                    sqlx::query("UPDATE events SET level=3,reason=$2,last_verification_at=now(),editorial_version=editorial_version+1 WHERE id=$1")
                        .bind(row.id).bind(reason).execute(&mut *tx).await.map_err(database_error)?;
                    let affected=sqlx::query("UPDATE jobs SET run_at=now()+interval '60 seconds',attempts=0,lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
                        .bind(job.id).bind(job.lease_token).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
                    if affected != 1 {
                        return Err(StoreError::LostLease);
                    }
                    outcome = EditorialOutcome::Deferred;
                }
            }
            EditorialStep::Verified(assessment)
                if state == EventState::Clustered
                    && assessment.level != Level::Observation
                    && !assessment.sources.is_empty() =>
            {
                sqlx::query("UPDATE events SET state='VERIFIED',level=$2,assessment_json=$3,verified_revision=content_revision,reason=NULL,last_verification_at=now(),editorial_version=editorial_version+1 WHERE id=$1")
                    .bind(row.id).bind(assessment.level.number()).bind(serde_json::to_value(assessment).map_err(|_|StoreError::InvalidJob)?).execute(&mut *tx).await.map_err(database_error)?;
                audit(
                    &mut tx,
                    row.id,
                    "event_verified",
                    json!({"revision":row.content_revision}),
                )
                .await?;
            }
            EditorialStep::Drafted(draft)
                if matches!(state, EventState::Verified | EventState::Drafted) =>
            {
                sqlx::query("UPDATE events SET state='DRAFTED',reason=NULL,title_pt=$2,summary_pt=$3,draft_json=$4,editorial_version=editorial_version+1 WHERE id=$1")
                    .bind(row.id).bind(&draft.title).bind(draft.summary.join("\n")).bind(serde_json::to_value(draft).map_err(|_|StoreError::InvalidJob)?).execute(&mut *tx).await.map_err(database_error)?;
                audit(
                    &mut tx,
                    row.id,
                    "draft_stored",
                    json!({"revision":row.content_revision,"attempt":row.editorial_attempts}),
                )
                .await?;
            }
            EditorialStep::Checked(report)
                if state == EventState::Drafted && row.review_id.is_none() =>
            {
                sqlx::query("UPDATE events SET state='CHECKED',reason=NULL,factcheck_json=$2,editorial_version=editorial_version+1 WHERE id=$1")
                    .bind(row.id).bind(serde_json::to_value(report).map_err(|_|StoreError::InvalidJob)?).execute(&mut *tx).await.map_err(database_error)?;
                sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('render',$1,$2) ON CONFLICT(dedupe_key) DO NOTHING")
                    .bind(format!("render:{}:{}",row.id,row.content_revision)).bind(json!({"event_id":row.id,"revision":row.content_revision})).execute(&mut *tx).await.map_err(database_error)?;
                audit(
                    &mut tx,
                    row.id,
                    "draft_checked",
                    json!({"revision":row.content_revision}),
                )
                .await?;
                finish(&mut tx, job).await?;
                outcome = EditorialOutcome::Finished;
            }
            EditorialStep::CheckedRevision { report, review }
                if state == EventState::Drafted && row.review_id.is_some() =>
            {
                if review.decision.publication_kind() != Some(row.publication_kind.as_str()) {
                    return Err(StoreError::InvalidOptions);
                }
                sqlx::query("UPDATE events SET state='CHECKED',reason=NULL,factcheck_json=$2,editorial_version=editorial_version+1 WHERE id=$1").bind(row.id).bind(json!({"report":report,"review":review})).execute(&mut *tx).await.map_err(database_error)?;
                sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('render',$1,$2) ON CONFLICT(dedupe_key) DO NOTHING").bind(format!("render:{}:{}",row.id,row.content_revision)).bind(json!({"event_id":row.id,"revision":row.content_revision})).execute(&mut *tx).await.map_err(database_error)?;
                audit(
                    &mut tx,
                    row.id,
                    "revision_draft_checked",
                    json!({"review_id":row.review_id,"revision":row.content_revision}),
                )
                .await?;
                finish(&mut tx, job).await?;
                outcome = EditorialOutcome::Finished;
            }
            EditorialStep::Rewrite { code, limit }
                if matches!(state, EventState::Verified | EventState::Drafted) && limit <= 2 =>
            {
                if row.editorial_attempts >= i32::from(limit) {
                    terminal(&mut tx, row.id, "REJECTED", "editorial_exhausted").await?;
                    finish(&mut tx, job).await?;
                    outcome = EditorialOutcome::Finished;
                } else {
                    sqlx::query("UPDATE events SET editorial_attempts=editorial_attempts+1,reason=$2,draft_json=NULL,factcheck_json=NULL,title_pt=NULL,summary_pt=NULL,editorial_version=editorial_version+1 WHERE id=$1")
                        .bind(row.id).bind(&code).execute(&mut *tx).await.map_err(database_error)?;
                }
                audit(&mut tx, row.id, "rewrite_requested", json!({"code":code})).await?;
            }
            EditorialStep::Failed { code } => {
                terminal(&mut tx, row.id, "FAILED", &code).await?;
                finish(&mut tx, job).await?;
                outcome = EditorialOutcome::Finished;
            }
            EditorialStep::Rejected { code } => {
                terminal(&mut tx, row.id, "REJECTED", &code).await?;
                finish(&mut tx, job).await?;
                outcome = EditorialOutcome::Finished;
            }
            EditorialStep::Skip => {
                finish(&mut tx, job).await?;
                outcome = EditorialOutcome::Finished;
            }
            _ => return Err(StoreError::Conflict),
        }
        tx.commit().await.map_err(database_error)?;
        Ok(outcome)
    }
}

async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    event: i64,
    revision: i64,
) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('verify',$1,$2) ON CONFLICT(dedupe_key) DO NOTHING")
        .bind(format!("verify:revision:{event}:{revision}")).bind(json!({"event_id":event})).execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}
async fn finish(tx: &mut Transaction<'_, Postgres>, job: &Job) -> Result<(), StoreError> {
    let affected=sqlx::query("UPDATE jobs SET done_at=now(),lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
        .bind(job.id).bind(job.lease_token).execute(&mut **tx).await.map_err(database_error)?.rows_affected();
    if affected != 1 {
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
async fn terminal(
    tx: &mut Transaction<'_, Postgres>,
    event: i64,
    state: &str,
    code: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "UPDATE events SET state=$2,reason=$3,editorial_version=editorial_version+1 WHERE id=$1",
    )
    .bind(event)
    .bind(state)
    .bind(code)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    audit(
        tx,
        event,
        "editorial_terminal",
        json!({"state":state,"code":code}),
    )
    .await
}
