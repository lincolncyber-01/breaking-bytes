//! CHECKED → RENDERED e outbox só depois de o JPEG estar durável em disco.

use crate::{EditorialContext, Job, Store, StoreError, database_error};
use serde_json::{Value, json};
use stories_core::EventState;
use stories_imaging::render::{Layout, TEMPLATE};

pub struct RenderArtifact {
    pub path: String,
    pub hash: [u8; 32],
    pub bytes: usize,
    pub layout: Layout,
    pub provenance: Value,
    pub dry_run: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub enum RenderDisposition {
    Stored,
    Obsolete,
    Rewritten,
    Rejected,
}

impl Store {
    pub async fn primary_image(&self, event: i64) -> Result<Option<String>, StoreError> {
        sqlx::query_scalar("SELECT a.image_url FROM events e LEFT JOIN articles a ON a.id=e.primary_article_id WHERE e.id=$1").bind(event).fetch_one(&self.pool).await.map_err(database_error)
    }

    pub async fn render_commit(
        &self,
        job: &Job,
        context: &EditorialContext,
        artifact: Option<RenderArtifact>,
        rewrite_limit: u8,
        code: &str,
    ) -> Result<RenderDisposition, StoreError> {
        if rewrite_limit > 2
            || artifact.is_none()
                && !matches!(code, "render_needs_rewrite" | "render_missing_glyph")
        {
            return Err(StoreError::InvalidOptions);
        }
        if artifact.as_ref().is_some_and(|a| {
            a.bytes == 0
                || a.bytes > 8 * 1024 * 1024
                || a.path.is_empty()
                || a.layout.template != TEMPLATE
        }) {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let owned:Option<i64>=sqlx::query_scalar("SELECT id FROM jobs WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp() AND done_at IS NULL AND dead_at IS NULL FOR UPDATE").bind(job.id).bind(job.lease_token).fetch_optional(&mut *tx).await.map_err(database_error)?;
        if owned.is_none() {
            return Err(StoreError::LostLease);
        }
        #[derive(sqlx::FromRow)]
        struct Row {
            state: String,
            content_revision: i64,
            editorial_version: i64,
            verified_revision: Option<i64>,
            editorial_attempts: i32,
        }
        let row:Row=sqlx::query_as("SELECT state,content_revision,editorial_version,verified_revision,editorial_attempts FROM events WHERE id=$1 FOR UPDATE").bind(context.event_id).fetch_one(&mut *tx).await.map_err(database_error)?;
        let current = EventState::parse(&row.state)?;
        let paused: bool = sqlx::query_scalar("SELECT paused FROM pipeline_control WHERE id=true")
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
        if paused {
            return Err(StoreError::Conflict);
        }
        if current != EventState::Checked
            || row.content_revision != context.revision
            || row.editorial_version != context.version
            || row.verified_revision != Some(context.revision)
        {
            // Uma revisão nova tem sua própria verificação; a resposta antiga não é aplicada.
            let affected=sqlx::query("UPDATE jobs SET done_at=now(),lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
                .bind(job.id).bind(job.lease_token).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
            if affected != 1 {
                return Err(StoreError::LostLease);
            }
            tx.commit().await.map_err(database_error)?;
            return Ok(RenderDisposition::Obsolete);
        }
        let blocked:bool=sqlx::query_scalar("SELECT event_is_blocked($1)").bind(context.event_id).fetch_one(&mut *tx).await.map_err(database_error)?;
        if blocked {
            sqlx::query("UPDATE events SET state='REJECTED',reason='policy_blocked' WHERE id=$1").bind(context.event_id).execute(&mut *tx).await.map_err(database_error)?;
            let n=sqlx::query("UPDATE jobs SET done_at=now(),lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()").bind(job.id).bind(job.lease_token).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
            if n!=1{return Err(StoreError::LostLease);}
            sqlx::query("INSERT INTO audit_log(event_id,step,detail) VALUES($1,'render_policy_blocked','{}')").bind(context.event_id).execute(&mut *tx).await.map_err(database_error)?;
            tx.commit().await.map_err(database_error)?;return Ok(RenderDisposition::Rejected);
        }
        if let Some(review) = &context.review {
            let mode = artifact
                .as_ref()
                .map(|a| a.dry_run)
                .unwrap_or(review.context.dry_run);
            let fresh: bool = sqlx::query_scalar("SELECT revision_evidence_fresh($1,$2,$3)")
                .bind(context.event_id)
                .bind(review.context.max_age_hours as i32)
                .bind(mode)
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
            if !fresh {
                return Err(StoreError::Conflict);
            }
        }
        let disposition = if let Some(artifact) = artifact {
            sqlx::query("INSERT INTO rendered_stories(event_id,revision,template,artifact_path,content_hash,byte_length,width,height,layout,image_provenance,dry_run) VALUES($1,$2,$3,$4,$5,$6,1080,1920,$7,$8,$9)")
                .bind(context.event_id).bind(context.revision).bind(TEMPLATE).bind(artifact.path).bind(artifact.hash.as_slice()).bind(i64::try_from(artifact.bytes).map_err(|_|StoreError::InvalidOptions)?)
                .bind(serde_json::to_value(artifact.layout).map_err(|_|StoreError::InvalidJob)?).bind(artifact.provenance).bind(artifact.dry_run).execute(&mut *tx).await.map_err(database_error)?;
            sqlx::query("UPDATE events SET state='RENDERED',editorial_version=editorial_version+1 WHERE id=$1").bind(context.event_id).execute(&mut *tx).await.map_err(database_error)?;
            sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('schedule',$1,$2) ON CONFLICT(dedupe_key) DO NOTHING").bind(format!("schedule:{}:{}",context.event_id,context.revision)).bind(json!({"event_id":context.event_id,"revision":context.revision,"dry_run":artifact.dry_run})).execute(&mut *tx).await.map_err(database_error)?;
            sqlx::query(
                "INSERT INTO audit_log(event_id,step,detail) VALUES($1,'story_rendered',$2)",
            )
            .bind(context.event_id)
            .bind(
                json!({"revision":context.revision,"template":TEMPLATE,"dry_run":artifact.dry_run}),
            )
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            RenderDisposition::Stored
        } else if row.editorial_attempts < i32::from(rewrite_limit) {
            sqlx::query("UPDATE events SET state='SUPERSEDED',reason=$2,editorial_version=editorial_version+1 WHERE id=$1").bind(context.event_id).bind(code).execute(&mut *tx).await.map_err(database_error)?;
            let child:i64=sqlx::query_scalar("INSERT INTO events(parent_event_id,primary_article_id,entities,publication_kind,review_id) SELECT parent_event_id,primary_article_id,entities,publication_kind,review_id FROM events WHERE id=$1 RETURNING id").bind(context.event_id).fetch_one(&mut *tx).await.map_err(database_error)?;
            sqlx::query("INSERT INTO event_articles(event_id,article_id) SELECT $2,article_id FROM event_articles WHERE event_id=$1").bind(context.event_id).bind(child).execute(&mut *tx).await.map_err(database_error)?;
            sqlx::query("UPDATE events SET state='CLUSTERED' WHERE id=$1")
                .bind(child)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            sqlx::query("UPDATE events SET editorial_attempts=parent.editorial_attempts+1,reason=$3 FROM events parent WHERE events.id=$2 AND parent.id=$1").bind(context.event_id).bind(child).bind(code).execute(&mut *tx).await.map_err(database_error)?;
            sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('verify',$1,$2)")
                .bind(format!("verify:layout:{child}"))
                .bind(json!({"event_id":child,"shorten":true}))
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO audit_log(event_id,step,detail) VALUES($1,'render_rewrite',$2)",
            )
            .bind(context.event_id)
            .bind(json!({"replacement":child,"code":code}))
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            RenderDisposition::Rewritten
        } else {
            sqlx::query("UPDATE events SET state='REJECTED',reason='layout_exhausted',editorial_version=editorial_version+1 WHERE id=$1").bind(context.event_id).execute(&mut *tx).await.map_err(database_error)?;
            sqlx::query(
                "INSERT INTO audit_log(event_id,step,detail) VALUES($1,'render_rejected',$2)",
            )
            .bind(context.event_id)
            .bind(json!({"code":code}))
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            RenderDisposition::Rejected
        };
        let affected=sqlx::query("UPDATE jobs SET done_at=now(),lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
            .bind(job.id).bind(job.lease_token).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        if affected != 1 {
            return Err(StoreError::LostLease);
        }
        tx.commit().await.map_err(database_error)?;
        Ok(disposition)
    }
}
