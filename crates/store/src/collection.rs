//! Cursor de coleta, artigos e outbox confirmados atomicamente.

use crate::{Job, Store, StoreError, database_error};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::FromRow;
use stories_cluster::{Match, classify};
use stories_normalize::NormalizedArticle;

#[derive(Debug, FromRow)]
pub struct SourceState {
    pub id: i32,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub failures: i32,
    pub blocked: bool,
    pub next_poll_at: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CollectionStats {
    pub inserted: u64,
    pub duplicates: u64,
}

#[derive(FromRow)]
struct StoredArticle {
    id: i64,
    url_canonical: String,
    title: String,
    title_hash: Vec<u8>,
    body_excerpt: Option<String>,
    image_url: Option<String>,
    published_at: Option<DateTime<Utc>>,
    entities: Vec<String>,
    facts: Vec<String>,
    tokens: Vec<String>,
}

impl StoredArticle {
    fn normalized(&self) -> Result<NormalizedArticle, StoreError> {
        let hash: [u8; 32] = self
            .title_hash
            .as_slice()
            .try_into()
            .map_err(|_| StoreError::InvalidJob)?;
        Ok(NormalizedArticle {
            url: self.url_canonical.clone(),
            title: self.title.clone(),
            title_hash: hash,
            excerpt: self.body_excerpt.clone().unwrap_or_default(),
            image_url: self.image_url.clone(),
            published_at: self.published_at,
            entities: self.entities.clone(),
            facts: self.facts.clone(),
            tokens: self.tokens.clone(),
        })
    }
}

impl Store {
    pub async fn register_source(
        &self,
        name: &str,
        layer: &str,
        kind: &str,
        url: &str,
        weight: i16,
        interval_s: i32,
    ) -> Result<i32, StoreError> {
        sqlx::query_scalar("INSERT INTO sources(name,layer,kind,url,weight,interval_s) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(name) DO UPDATE SET layer=EXCLUDED.layer, kind=EXCLUDED.kind, url=EXCLUDED.url, weight=EXCLUDED.weight, interval_s=EXCLUDED.interval_s, enabled=true, etag=CASE WHEN sources.url=EXCLUDED.url AND sources.kind=EXCLUDED.kind THEN sources.etag ELSE NULL END, last_modified=CASE WHEN sources.url=EXCLUDED.url AND sources.kind=EXCLUDED.kind THEN sources.last_modified ELSE NULL END RETURNING id")
            .bind(name).bind(layer).bind(kind).bind(url).bind(weight).bind(interval_s).fetch_one(&self.pool).await.map_err(database_error)
    }

    pub async fn source_state(&self, id: i32) -> Result<SourceState, StoreError> {
        sqlx::query_as(
            "SELECT id,etag,last_modified,failures,next_poll_at,source_is_blocked(id) AS blocked FROM sources WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)
    }

    /// Artigos, proveniência, jobs e validadores HTTP nunca se separam por uma queda.
    pub async fn ingest_batch(
        &self,
        source: i32,
        articles: &[NormalizedArticle],
        etag: Option<&str>,
        last_modified: Option<&str>,
        not_modified: bool,
        next_poll: DateTime<Utc>,
    ) -> Result<CollectionStats, StoreError> {
        if articles.len() > 200 {
            return Err(StoreError::InvalidOptions);
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let mut stats = CollectionStats::default();
        let blocked: bool = sqlx::query_scalar("SELECT source_is_blocked($1)")
            .bind(source)
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
        if blocked {
            tx.commit().await.map_err(database_error)?;
            return Ok(stats);
        }
        for article in articles {
            let inserted:Option<i64>=sqlx::query_scalar("INSERT INTO articles(source_id,url_canonical,title,title_hash,body_excerpt,image_url,published_at,entities,facts,tokens) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(url_canonical) DO NOTHING RETURNING id")
                .bind(source).bind(&article.url).bind(&article.title).bind(article.title_hash.as_slice()).bind(&article.excerpt).bind(&article.image_url).bind(article.published_at).bind(&article.entities).bind(&article.facts).bind(&article.tokens)
                .fetch_optional(&mut *tx).await.map_err(database_error)?;
            let id = if let Some(id) = inserted {
                stats.inserted += 1;
                id
            } else {
                stats.duplicates += 1;
                sqlx::query_scalar("SELECT id FROM articles WHERE url_canonical=$1")
                    .bind(&article.url)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(database_error)?
            };
            sqlx::query("INSERT INTO article_sightings(article_id,source_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(id).bind(source).execute(&mut *tx).await.map_err(database_error)?;
            if inserted.is_some() {
                sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('cluster',$1,$2) ON CONFLICT(dedupe_key) DO NOTHING")
                    .bind(format!("cluster:{id}")).bind(json!({"article_id":id})).execute(&mut *tx).await.map_err(database_error)?;
            }
        }
        sqlx::query("UPDATE sources SET etag=CASE WHEN $4 THEN COALESCE($2,etag) ELSE $2 END, last_modified=CASE WHEN $4 THEN COALESCE($3,last_modified) ELSE $3 END, last_polled_at=now(),last_success_at=now(),failures=0,last_error_code=NULL,next_poll_at=$5 WHERE id=$1")
            .bind(source).bind(etag).bind(last_modified).bind(not_modified).bind(next_poll).execute(&mut *tx).await.map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(stats)
    }

    pub async fn source_failed(
        &self,
        source: i32,
        code: &str,
        next_poll: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        if !matches!(
            code,
            "http" | "parse" | "limits" | "kind" | "worker" | "normalization"
        ) {
            return Err(StoreError::InvalidOptions);
        }
        sqlx::query("UPDATE sources SET last_polled_at=now(), failures=LEAST(failures+1,32),last_error_code=$2,next_poll_at=$3 WHERE id=$1")
            .bind(source).bind(code).bind(next_poll).execute(&self.pool).await.map_err(database_error)?;
        Ok(())
    }

    pub async fn cluster_job(
        &self,
        job: &Job,
        threshold: f64,
        window_hours: u32,
    ) -> Result<i64, StoreError> {
        if !(0.0..=1.0).contains(&threshold)
            || !(1..=168).contains(&window_hours)
            || job.kind != "cluster"
        {
            return Err(StoreError::InvalidOptions);
        }
        let article_id = job
            .payload
            .get("article_id")
            .and_then(|v| v.as_i64())
            .ok_or(StoreError::InvalidJob)?;
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let owned:Option<i64>=sqlx::query_scalar("SELECT id FROM jobs WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp() AND done_at IS NULL AND dead_at IS NULL FOR UPDATE")
            .bind(job.id).bind(job.lease_token).fetch_optional(&mut *tx).await.map_err(database_error)?;
        if owned.is_none() {
            return Err(StoreError::LostLease);
        }
        // Serialização curta evita criar dois eventos quando trabalhadores descobrem o mesmo fato juntos.
        sqlx::query("SELECT pg_advisory_xact_lock(7281934201)")
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let article:StoredArticle=sqlx::query_as("SELECT id,url_canonical,title,title_hash,body_excerpt,image_url,published_at,entities,facts,tokens FROM articles WHERE id=$1")
            .bind(article_id).fetch_one(&mut *tx).await.map_err(database_error)?;
        let normalized = article.normalized()?;
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT event_id FROM event_articles WHERE article_id=$1 ORDER BY event_id LIMIT 1",
        )
        .bind(article_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        let mut target = existing;
        let mut parent = None;
        if target.is_none() {
            let candidates:Vec<(i64,String,i64)>=sqlx::query_as("SELECT e.id,e.state,e.primary_article_id FROM events e JOIN articles a ON a.id=e.primary_article_id WHERE e.state NOT IN ('REJECTED','SUPERSEDED','FAILED','NEEDS_CORRECTION') AND e.created_at>=now()-make_interval(hours=>$1) AND (a.title_hash=$2 OR a.entities && $3::text[]) ORDER BY e.created_at,e.id LIMIT 200")
                .bind(i32::try_from(window_hours).map_err(|_|StoreError::InvalidOptions)?).bind(article.title_hash.as_slice()).bind(&article.entities).fetch_all(&mut *tx).await.map_err(database_error)?;
            for (event, state, primary_id) in candidates {
                let primary:StoredArticle=sqlx::query_as("SELECT id,url_canonical,title,title_hash,body_excerpt,image_url,published_at,entities,facts,tokens FROM articles WHERE id=$1")
                    .bind(primary_id).fetch_one(&mut *tx).await.map_err(database_error)?;
                match classify(&primary.normalized()?, &normalized, threshold) {
                    Match::Different => {}
                    Match::Same => {
                        target = Some(event);
                        break;
                    }
                    Match::NewFacts if state == "PUBLISHED" => {
                        parent = Some(event);
                    }
                    Match::NewFacts => {
                        target = Some(event);
                        break;
                    }
                }
            }
        }
        let event = if let Some(event) = target {
            event
        } else {
            let event: i64 = sqlx::query_scalar(
                "INSERT INTO events(parent_event_id,primary_article_id) VALUES($1,$2) RETURNING id",
            )
            .bind(parent)
            .bind(article.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
            sqlx::query("UPDATE events SET state='CLUSTERED',entities=$2 WHERE id=$1")
                .bind(event)
                .bind(json!(article.entities))
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            sqlx::query(
                "INSERT INTO audit_log(event_id,step,detail) VALUES($1,'cluster_created',$2)",
            )
            .bind(event)
            .bind(json!({"article_id":article.id,"parent_event_id":parent,"mode":"hash_entities"}))
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
            event
        };
        sqlx::query(
            "INSERT INTO event_articles(event_id,article_id) VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(event)
        .bind(article.id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        // Novo artigo permite reavaliar um evento N3 que ganhou confirmação.
        sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) SELECT 'verify',$2,$3 FROM events WHERE id=$1 AND state='CLUSTERED' ON CONFLICT(dedupe_key) DO NOTHING")
            .bind(event).bind(format!("verify:{event}:{}",article.id)).bind(json!({"event_id":event})).execute(&mut *tx).await.map_err(database_error)?;
        // Fonte primária: maior peso; desempate por detecção, depois ID estável.
        sqlx::query("UPDATE events SET primary_article_id=(SELECT a.id FROM event_articles ea JOIN articles a ON a.id=ea.article_id JOIN article_sightings sight ON sight.article_id=a.id JOIN sources s ON s.id=sight.source_id WHERE ea.event_id=$1 ORDER BY s.weight DESC,a.detected_at,a.id LIMIT 1) WHERE id=$1 AND state<>'PUBLISHED'")
            .bind(event).execute(&mut *tx).await.map_err(database_error)?;
        sqlx::query(
            "INSERT INTO audit_log(event_id,step,detail) VALUES($1,'article_clustered',$2)",
        )
        .bind(event)
        .bind(json!({"article_id":article.id}))
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        let affected=sqlx::query("UPDATE jobs SET done_at=now(),lease_token=NULL,lease_until=NULL,locked_at=NULL WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp()")
            .bind(job.id).bind(job.lease_token).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
        if affected != 1 {
            return Err(StoreError::LostLease);
        }
        tx.commit().await.map_err(database_error)?;
        Ok(event)
    }
}
