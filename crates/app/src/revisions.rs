//! Acompanhamento das fontes publicadas e revisão estruturada, sempre sob lease.

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use std::{future::Future, sync::Arc, time::Duration};
use stories_collectors::{Collector, SourceKind, http::HttpClient};
use stories_config::Settings;
use stories_core::CapturedRevision;
use stories_editorial::{
    Editorial, Task,
    client::{CompletionClient, LlmError},
    revisions::{ChangeDecision, ChangeReview},
};
use stories_store::{Job, JobFailure, SourceObservation, Store, StoreError, WatchPolicy};
use stories_verify::{Evidence, Level};
use tokio::sync::watch;

trait RevisionReader: Send + Sync {
    fn read(
        &self,
        evidence: &Evidence,
    ) -> impl Future<Output = Result<Option<CapturedRevision>, &'static str>> + Send;
}
impl RevisionReader for Collector {
    async fn read(&self, evidence: &Evidence) -> Result<Option<CapturedRevision>, &'static str> {
        let kind = SourceKind::parse(&evidence.kind).map_err(|e| e.code())?;
        self.revisit(
            kind,
            &evidence.source_url,
            &evidence.article_url,
            Utc::now(),
        )
        .await
        .map_err(|e| e.code())
    }
}
fn policy(settings: &Settings) -> WatchPolicy {
    let c = &settings.corrections;
    WatchPolicy {
        window_hours: c.window_hours,
        high_impact_hours: c.high_impact_hours,
        high_impact_score: c.high_impact_score,
        interval_s: c.interval_s,
        max_sources: c.max_sources,
    }
}
async fn failure(
    settings: &Settings,
    store: &Store,
    job: &Job,
    error: anyhow::Error,
) -> Result<()> {
    if matches!(
        error.downcast_ref::<StoreError>(),
        Some(StoreError::LostLease)
    ) {
        return Ok(());
    }
    let failure = if matches!(
        error.downcast_ref::<StoreError>(),
        Some(StoreError::InvalidJob | StoreError::InvalidOptions)
    ) {
        JobFailure::Permanent
    } else {
        JobFailure::Transient
    };
    match store
        .retry(job, failure, settings.database.max_attempts)
        .await
    {
        Ok(()) | Err(StoreError::LostLease) => {}
        Err(e) => return Err(e.into()),
    }
    tracing::warn!(
        job_id = job.id,
        "reconsulta ou revisão reagendada ou isolada"
    );
    Ok(())
}
pub async fn watch(
    settings: Settings,
    store: Store,
    http: HttpClient,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    // Fontes antigas continuam antigas; o limite amplo permite ler a janela de acompanhamento.
    let collector = Collector::new(http, settings.collection.max_items, 168)?;
    let lease = Duration::from_secs(settings.database.lease_s);
    let mut maintenance = tokio::time::Instant::now();
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= maintenance {
            if !store.paused().await? {
                store
                    .enroll_watches(policy(&settings), settings.instagram.dry_run)
                    .await?;
            }
            maintenance = tokio::time::Instant::now() + Duration::from_secs(60);
        }
        let Some(job) = store
            .claim_kind(lease, settings.database.max_attempts, "revisit")
            .await?
        else {
            tokio::select! { _=stop.changed()=>{}, _=tokio::time::sleep(Duration::from_millis(250))=>{} }
            continue;
        };
        let work = observe(&settings, &store, &job, &collector);
        match crate::editorial::owned_work(&store, &job, lease, &mut stop, work).await {
            Some(Ok(())) => {}
            Some(Err(e)) => failure(&settings, &store, &job, e).await?,
            None => {
                let _ = store
                    .retry(&job, JobFailure::Transient, settings.database.max_attempts)
                    .await;
                return Ok(());
            }
        }
    }
}
async fn observe<R: RevisionReader>(
    settings: &Settings,
    store: &Store,
    job: &Job,
    reader: &R,
) -> Result<()> {
    let id = job
        .payload
        .get("publication_id")
        .and_then(Value::as_i64)
        .filter(|v| *v > 0)
        .ok_or(StoreError::InvalidJob)?;
    let context = store.watch_context(id).await?;
    if !context.active {
        store.complete(job).await?;
        return Ok(());
    }
    if context.dry_run != settings.instagram.dry_run || store.paused().await? {
        store
            .defer_revision_watch(job, settings.corrections.interval_s)
            .await?;
        return Ok(());
    }
    let mut observations = Vec::new();
    // Serial e limitado a quatro fontes por publicação; cada HTTP mantém os limites do coletor.
    for source in &context.sources {
        let result = if source.active {
            reader.read(&source.baseline).await
        } else {
            Err("identity_changed")
        };
        let (captured, error) = match result {
            Ok(captured) => (captured, None),
            Err(code) => (None, Some(code)),
        };
        observations.push(SourceObservation {
            article_id: source.baseline.article_id,
            source_id: source.baseline.source_id,
            captured,
            error,
        });
    }
    store
        .observe_revision(
            job,
            &context,
            observations,
            settings.corrections.interval_s,
            settings.pipeline.max_age_hours,
        )
        .await?;
    Ok(())
}
pub async fn review<C: CompletionClient>(
    settings: Settings,
    store: Store,
    client: C,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let engine = Arc::new(
        tokio::task::spawn_blocking(Editorial::new)
            .await
            .context("inicialização da revisão falhou")??,
    );
    let lease = Duration::from_secs(settings.database.lease_s);
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let Some(job) = store
            .claim_kind(lease, settings.database.max_attempts, "review_revision")
            .await?
        else {
            tokio::select! { _=stop.changed()=>{}, _=tokio::time::sleep(Duration::from_millis(250))=>{} }
            continue;
        };
        let work = review_one(&settings, &store, &job, &client, engine.clone());
        match crate::editorial::owned_work(&store, &job, lease, &mut stop, work).await {
            Some(Ok(())) => {}
            Some(Err(e)) => failure(&settings, &store, &job, e).await?,
            None => {
                let _ = store
                    .retry(&job, JobFailure::Transient, settings.database.max_attempts)
                    .await;
                return Ok(());
            }
        }
    }
}
async fn review_one<C: CompletionClient>(
    settings: &Settings,
    store: &Store,
    job: &Job,
    client: &C,
    engine: Arc<Editorial>,
) -> Result<()> {
    let id = job
        .payload
        .get("review_id")
        .and_then(Value::as_i64)
        .filter(|v| *v > 0)
        .ok_or(StoreError::InvalidJob)?;
    let record = store.review_record(id).await?;
    if record.status != "pending" {
        store.complete(job).await?;
        return Ok(());
    }
    if store.paused().await? {
        store
            .retry(job, JobFailure::Transient, settings.database.max_attempts)
            .await?;
        return Ok(());
    }
    let age = settings.pipeline.max_age_hours;
    let assessment = record.context.assessment(Utc::now(), age);
    let mut review = ChangeReview {
        decision: ChangeDecision::Uncertain,
        citations: Vec::new(),
    };
    if assessment.level != Level::Observation
        && record.context.dry_run == settings.instagram.dry_run
    {
        match client
            .complete(Task::ReviewChange, record.context.input(&assessment))
            .await
        {
            Ok(value) => {
                let context = record.context.clone();
                let result = tokio::task::spawn_blocking(move || {
                    engine.parse_change(value, &context, &assessment, Utc::now(), age)
                })
                .await?;
                match result {
                    Ok(value) => review = value,
                    Err(_) if job.attempts <= i32::from(settings.pipeline.max_rewrite_attempts) => {
                        store
                            .retry(job, JobFailure::Transient, settings.database.max_attempts)
                            .await?;
                        return Ok(());
                    }
                    Err(_) => {}
                }
            }
            Err(e) if e.transient() && job.attempts < settings.database.max_attempts => {
                store
                    .retry_delayed(
                        job,
                        JobFailure::Transient,
                        settings.database.max_attempts,
                        Duration::from_secs(e.retry_after().unwrap_or(0)),
                    )
                    .await?;
                return Ok(());
            }
            Err(LlmError::Input) => return Err(StoreError::InvalidJob.into()),
            Err(_) => {}
        }
    }
    store
        .apply_revision_review(job, &record, review, age)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
