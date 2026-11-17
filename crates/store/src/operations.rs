//! Operações administrativas idempotentes e auditadas, exclusivamente sob pausa global.
use crate::{Store, StoreError, database_error};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::{FromRow, Postgres, Transaction};
use stories_core::policy::{ResolutionRequest, RuleChange};

#[derive(Serialize, FromRow)]
pub struct RuleRecord {
    pub id: i32,
    pub kind: String,
    pub value: String,
    pub active: bool,
}
#[derive(Serialize)]
pub struct RulePage {
    pub items: Vec<RuleRecord>,
    pub next_id: Option<i32>,
}
pub struct ResolutionWitness {
    pub account: String,
    pub container_id: String,
    pub media_id: String,
    pub observed_at: DateTime<Utc>,
    pub container_published: bool,
    pub story_visible: bool,
}
#[derive(Serialize)]
pub struct ResolutionResult {
    pub publication_id: i64,
    pub media_id: String,
    pub replayed: bool,
}
#[derive(FromRow)]
struct ResolutionPublication {
    event_id: i64,
    status: String,
    container_id: Option<String>,
    dry_run: bool,
    account_key: Option<String>,
    request_started_at: Option<DateTime<Utc>>,
}
async fn paused(tx: &mut Transaction<'_, Postgres>) -> Result<(), StoreError> {
    let value: bool =
        sqlx::query_scalar("SELECT paused FROM pipeline_control WHERE id=true FOR UPDATE")
            .fetch_one(&mut **tx)
            .await
            .map_err(database_error)?;
    if !value {
        return Err(StoreError::Conflict);
    }
    Ok(())
}
fn digest(publication: i64, request: &ResolutionRequest) -> Result<[u8; 32], StoreError> {
    let bytes = serde_json::to_vec(&json!({"publication_id":publication,"request":request}))
        .map_err(|_| StoreError::InvalidOptions)?;
    Ok(*blake3::hash(&bytes).as_bytes())
}
impl Store {
    pub async fn rules(&self, after: i32) -> Result<RulePage, StoreError> {
        if after < 0 {
            return Err(StoreError::InvalidOptions);
        }
        let mut items: Vec<RuleRecord> = sqlx::query_as(
            "SELECT id,kind,value,active FROM blocklist WHERE id>$1 ORDER BY id LIMIT 201",
        )
        .bind(after)
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        let next_id = if items.len() > 200 {
            items.truncate(200);
            items.last().map(|r| r.id)
        } else {
            None
        };
        Ok(RulePage { items, next_id })
    }
    pub async fn change_rule(&self, change: &RuleChange) -> Result<(), StoreError> {
        change.validate().map_err(|_| StoreError::InvalidOptions)?;
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        paused(&mut tx).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM blocklist WHERE active")
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
        let row:Option<(i32,bool)>=sqlx::query_as("SELECT id,active FROM blocklist WHERE kind=$1 AND policy_text(value)=policy_text($2) ORDER BY active DESC,id LIMIT 1 FOR UPDATE").bind(change.kind.as_str()).bind(&change.value).fetch_optional(&mut *tx).await.map_err(database_error)?;
        if change.blocked && count >= 1000 && row.as_ref().is_none_or(|(_, active)| !*active) {
            return Err(StoreError::InvalidOptions);
        }
        if let Some((id, _)) = row {
            sqlx::query("UPDATE blocklist SET active=false,updated_at=now() WHERE kind=$1 AND policy_text(value)=policy_text($2)").bind(change.kind.as_str()).bind(&change.value).execute(&mut *tx).await.map_err(database_error)?;
            if change.blocked {
                sqlx::query("UPDATE blocklist SET active=true,updated_at=now() WHERE id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
        } else if change.blocked {
            sqlx::query("INSERT INTO blocklist(kind,value,active) VALUES($1,$2,true)")
                .bind(change.kind.as_str())
                .bind(change.value.trim())
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        }
        sqlx::query("INSERT INTO audit_log(step,detail) VALUES('blocklist_changed',$1)").bind(json!({"kind":change.kind,"blocked":change.blocked,"rule_hash":blake3::hash(change.value.as_bytes()).to_hex().to_string()})).execute(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)
    }
    pub async fn event_blocked(&self, event: i64) -> Result<bool, StoreError> {
        sqlx::query_scalar("SELECT event_is_blocked($1)")
            .bind(event)
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)
    }
    pub async fn resolution_receipt(
        &self,
        publication: i64,
        request: &ResolutionRequest,
    ) -> Result<Option<ResolutionResult>, StoreError> {
        request.validate().map_err(|_| StoreError::InvalidOptions)?;
        let row:Option<(i64,Vec<u8>,String)>=sqlx::query_as("SELECT publication_id,payload_hash,media_id FROM publication_resolution_receipts WHERE request_id=$1").bind(request.request_id).fetch_optional(&self.pool).await.map_err(database_error)?;
        if let Some((id, hash, media)) = row {
            if id != publication || hash != digest(publication, request)? {
                return Err(StoreError::Conflict);
            }
            return Ok(Some(ResolutionResult {
                publication_id: id,
                media_id: media,
                replayed: true,
            }));
        }
        Ok(None)
    }
    pub async fn resolve_publication(
        &self,
        publication: i64,
        request: &ResolutionRequest,
        witness: ResolutionWitness,
    ) -> Result<ResolutionResult, StoreError> {
        request.validate().map_err(|_| StoreError::InvalidOptions)?;
        if let Some(result) = self.resolution_receipt(publication, request).await? {
            return Ok(result);
        }
        if publication <= 0
            || !witness.container_published
            || !witness.story_visible
            || witness.media_id != request.media_id
            || witness.container_id != request.expected_container_id
            || witness.observed_at > Utc::now()
            || Utc::now() - witness.observed_at > chrono::Duration::seconds(30)
        {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(&witness.account)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        paused(&mut tx).await?;
        let repeated:Option<(i64,Vec<u8>,String)>=sqlx::query_as("SELECT publication_id,payload_hash,media_id FROM publication_resolution_receipts WHERE request_id=$1").bind(request.request_id).fetch_optional(&mut *tx).await.map_err(database_error)?;
        if let Some((id, hash, media)) = repeated {
            if id != publication || hash != digest(publication, request)? {
                return Err(StoreError::Conflict);
            }
            tx.commit().await.map_err(database_error)?;
            return Ok(ResolutionResult {
                publication_id: id,
                media_id: media,
                replayed: true,
            });
        }
        let row: ResolutionPublication = sqlx::query_as("SELECT event_id,status,container_id,dry_run,account_key,request_started_at FROM publications WHERE id=$1 FOR UPDATE").bind(publication).fetch_one(&mut *tx).await.map_err(database_error)?;
        let ResolutionPublication {
            event_id: event,
            status,
            container_id: container,
            dry_run: dry,
            account_key: account,
            request_started_at: started,
        } = row;
        if dry
            || account.as_deref() != Some(witness.account.as_str())
            || status != request.expected_status
            || container.as_deref() != Some(request.expected_container_id.as_str())
            || started.is_none()
        {
            return Err(StoreError::Conflict);
        }
        let jobs:Vec<(i64,bool)>=sqlx::query_as("SELECT id,COALESCE(lease_until>clock_timestamp(),false) FROM jobs WHERE payload->>'publication_id'=$1 AND done_at IS NULL AND dead_at IS NULL ORDER BY id FOR UPDATE NOWAIT").bind(publication.to_string()).fetch_all(&mut *tx).await.map_err(|e|if e.as_database_error().and_then(|d|d.code()).as_deref()==Some("55P03"){StoreError::Conflict}else{database_error(e)})?;
        if jobs.iter().any(|(_, active)| *active) {
            return Err(StoreError::Conflict);
        }
        sqlx::query("UPDATE publications SET status='published',media_id=$2,published_at=COALESCE(published_at,$3),remote_confirmed_at=COALESCE(remote_confirmed_at,$3),reconciled_at=now() WHERE id=$1").bind(publication).bind(&request.media_id).bind(witness.observed_at).execute(&mut *tx).await.map_err(|e|if e.as_database_error().and_then(|d|d.code()).as_deref()==Some("23505"){StoreError::Conflict}else{database_error(e)})?;
        sqlx::query("UPDATE events SET state='PUBLISHED' WHERE id=$1 AND state='PUBLISHING'")
            .bind(event)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        for (id, _) in jobs {
            sqlx::query("UPDATE jobs SET done_at=now(),lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1").bind(id).execute(&mut *tx).await.map_err(database_error)?;
        }
        sqlx::query("UPDATE publication_watches SET superseded_by=$2 WHERE publication_id=(SELECT r.publication_id FROM events e JOIN publication_reviews r ON r.id=e.review_id WHERE e.id=$1) AND superseded_by IS NULL").bind(event).bind(publication).execute(&mut *tx).await.map_err(database_error)?;
        sqlx::query("INSERT INTO publication_resolution_receipts(request_id,publication_id,payload_hash,media_id) VALUES($1,$2,$3,$4)").bind(request.request_id).bind(publication).bind(digest(publication,request)?.as_slice()).bind(&request.media_id).execute(&mut *tx).await.map_err(database_error)?;
        sqlx::query("INSERT INTO audit_log(event_id,step,detail) VALUES($1,'publication_resolved_by_operator',$2)").bind(event).bind(json!({"request_id":request.request_id,"publication_id":publication,"media_id":request.media_id,"previous_status":status,"association_origin":"operator_attestation","container_published":true,"story_visible":true})).execute(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(ResolutionResult {
            publication_id: publication,
            media_id: request.media_id.clone(),
            replayed: false,
        })
    }
}
