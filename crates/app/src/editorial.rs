//! Worker serial, retomável e protegido por lease durante HTTP e CPU.

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{Value, json};
use std::{future::Future, sync::Arc, time::Duration};
use stories_collectors::{Collector, SourceKind, http::HttpClient};
use stories_config::Settings;
use stories_core::EventState;
use stories_editorial::{
    Editorial, Task,
    client::{AnthropicClient, CompletionClient, LlmError},
};
use stories_store::{EditorialOutcome, EditorialStep, Job, JobFailure, Store, StoreError};
use stories_verify::{Evidence, Level};
use tokio::sync::watch;

trait EvidenceClient: Send + Sync {
    fn hydrate(&self, evidence: Vec<Evidence>) -> impl Future<Output = Vec<Evidence>> + Send;
}

impl EvidenceClient for Collector {
    async fn hydrate(&self, evidence: Vec<Evidence>) -> Vec<Evidence> {
        let mut output = Vec::new();
        let mut visited = std::collections::BTreeSet::new();
        for mut source in evidence {
            if source.layer == "C"
                || !visited.insert(source.article_id)
                || output.len() >= 8
                || !stories_verify::source_matches(&source)
            {
                continue;
            }
            let Ok(kind) = SourceKind::parse(&source.kind) else {
                continue;
            };
            match self
                .confirm(kind, &source.source_url, &source.article_url, Utc::now())
                .await
            {
                Ok(Some(article)) => {
                    // A data original vence updated/lastmod e o horário de detecção.
                    source.date_confirmed = source
                        .published_at
                        .zip(article.published_at)
                        .is_some_and(|(stored, fresh)| (stored - fresh).num_seconds().abs() <= 300);
                    source.published_at = article.published_at;
                    source.title = article.title;
                    source.excerpt = article.excerpt;
                    output.push(source);
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    source_id = source.source_id,
                    code = error.code(),
                    "evidência indisponível nesta tentativa"
                ),
            }
        }
        output
    }
}

pub async fn run(
    settings: Settings,
    store: Store,
    http: HttpClient,
    client: AnthropicClient,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let collector = Collector::new(
        http,
        settings.collection.max_items,
        settings.pipeline.max_age_hours,
    )?;
    let engine = Arc::new(
        tokio::task::spawn_blocking(Editorial::new)
            .await
            .context("inicialização editorial falhou")??,
    );
    let lease = Duration::from_secs(settings.database.lease_s);
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let Some(job) = store
            .claim_kind(lease, settings.database.max_attempts, "verify")
            .await?
        else {
            tokio::select! {_=stop.changed()=>{},_=tokio::time::sleep(Duration::from_millis(250))=>{}}
            continue;
        };
        let process = process(&settings, &store, &job, &client, &collector, engine.clone());
        tokio::pin!(process);
        let mut heartbeat = tokio::time::interval(lease / 3);
        heartbeat.tick().await;
        let result = loop {
            tokio::select! {
                result=&mut process=>break result,
                _=heartbeat.tick()=>{
                    if let Err(error)=store.renew(&job,lease).await {break Err(error.into());}
                }
                _=stop.changed()=>{
                    let _=store.retry(&job,JobFailure::Transient,settings.database.max_attempts).await;
                    return Ok(());
                }
            }
        };
        if let Err(error) = result {
            if matches!(
                error.downcast_ref::<StoreError>(),
                Some(StoreError::LostLease)
            ) {
                tracing::warn!(
                    job_id = job.id,
                    "lease editorial perdido; resultado descartado"
                );
            } else {
                // A mensagem de erro externa não entra no log nem no banco.
                let failure = if matches!(
                    error.downcast_ref::<StoreError>(),
                    Some(StoreError::InvalidJob)
                ) {
                    JobFailure::Permanent
                } else {
                    JobFailure::Transient
                };
                match store
                    .retry(&job, failure, settings.database.max_attempts)
                    .await
                {
                    Ok(()) | Err(StoreError::LostLease) => {}
                    Err(error) => return Err(error.into()),
                }
                tracing::warn!(job_id = job.id, "trabalho editorial reagendado ou isolado");
            }
        }
    }
}

async fn process<C: CompletionClient, E: EvidenceClient>(
    settings: &Settings,
    store: &Store,
    job: &Job,
    client: &C,
    evidence_client: &E,
    engine: Arc<Editorial>,
) -> Result<()> {
    let event = job
        .payload
        .get("event_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .ok_or(StoreError::InvalidJob)?;
    loop {
        let context = store.editorial_context(event).await?;
        if store.paused().await? {
            store
                .retry(job, JobFailure::Transient, settings.database.max_attempts)
                .await?;
            return Ok(());
        }
        if context
            .verified_revision
            .is_some_and(|revision| revision != context.revision)
        {
            store
                .editorial_commit(job, &context, EditorialStep::Skip)
                .await?;
            return Ok(());
        }
        let step = match context.state {
            EventState::Clustered => {
                let evidence = evidence_client.hydrate(context.evidence.clone()).await;
                let age = settings.pipeline.max_age_hours;
                let assessment = tokio::task::spawn_blocking(move || {
                    stories_verify::assess(&evidence, Utc::now(), age)
                })
                .await?;
                if assessment.level == Level::Observation {
                    EditorialStep::Observe {
                        reason: assessment.reason,
                        expired: Utc::now() - context.created_at
                            >= chrono::Duration::hours(i64::from(age)),
                    }
                } else {
                    EditorialStep::Verified(assessment)
                }
            }
            EventState::Verified | EventState::Drafted => {
                let assessment = context.assessment.clone().ok_or(StoreError::InvalidJob)?;
                let saved = assessment.sources.clone();
                let age = settings.pipeline.max_age_hours;
                if tokio::task::spawn_blocking(move || {
                    stories_verify::assess(&saved, Utc::now(), age)
                })
                .await?
                .level
                    == Level::Observation
                {
                    store
                        .editorial_commit(
                            job,
                            &context,
                            EditorialStep::Rejected {
                                code: "evidence_expired".into(),
                            },
                        )
                        .await?;
                    return Ok(());
                }
                match context.draft.clone() {
                    None => {
                        let data = json!({"evidence":assessment,"kind":context.kind,"rewrite_number":context.rewrites,"retry_reason":context.reason,"style":{"max_title_words":8,"summary_lines":[2,4],"language":"pt-BR"}});
                        match client.complete(Task::Draft, data).await {
                            Ok(value) => {
                                let kind = context.kind;
                                let validator = engine.clone();
                                match tokio::task::spawn_blocking(move || {
                                    let draft = validator.parse_draft(value, &assessment, kind)?;
                                    validator.deterministic(&draft, &assessment)?;
                                    Ok::<_, stories_editorial::EditorialError>(draft)
                                })
                                .await?
                                {
                                    Ok(draft) => EditorialStep::Drafted(draft),
                                    Err(error) => rewrite(settings, error.code()),
                                }
                            }
                            Err(error) => match llm_error(settings, store, job, error).await? {
                                Some(step) => step,
                                None => return Ok(()),
                            },
                        }
                    }
                    Some(draft) => {
                        let validator = engine.clone();
                        let a = assessment.clone();
                        let d = draft.clone();
                        let validation =
                            tokio::task::spawn_blocking(move || validator.deterministic(&d, &a))
                                .await?;
                        if let Err(error) = validation {
                            rewrite(settings, error.code())
                        } else {
                            let data = json!({"evidence":assessment,"segments":draft.segments(),"kind":context.kind});
                            match client.complete(Task::Check, data).await {
                                Ok(value) => {
                                    let validator = engine.clone();
                                    match tokio::task::spawn_blocking(move || {
                                        validator.parse_report(value, &draft, &assessment)
                                    })
                                    .await?
                                    {
                                        Ok(report) => EditorialStep::Checked(report),
                                        Err(error) => rewrite(settings, error.code()),
                                    }
                                }
                                Err(error) => match llm_error(settings, store, job, error).await? {
                                    Some(step) => step,
                                    None => return Ok(()),
                                },
                            }
                        }
                    }
                }
            }
            _ => EditorialStep::Skip,
        };
        match store.editorial_commit(job, &context, step).await? {
            EditorialOutcome::Progress => {}
            _ => return Ok(()),
        }
    }
}

fn rewrite(settings: &Settings, code: &str) -> EditorialStep {
    EditorialStep::Rewrite {
        code: code.into(),
        limit: settings.pipeline.max_rewrite_attempts,
    }
}

async fn llm_error(
    settings: &Settings,
    store: &Store,
    job: &Job,
    error: LlmError,
) -> Result<Option<EditorialStep>> {
    let code = error.code();
    tracing::warn!(
        job_id = job.id,
        code,
        "provedor editorial não produziu saída válida"
    );
    if error.transient() && job.attempts < settings.database.max_attempts {
        store
            .retry_delayed(
                job,
                JobFailure::Transient,
                settings.database.max_attempts,
                Duration::from_secs(error.retry_after().unwrap_or(0)),
            )
            .await?;
        Ok(None)
    } else if matches!(error, LlmError::Refusal) {
        Ok(Some(EditorialStep::Rejected { code: code.into() }))
    } else if matches!(error, LlmError::Response | LlmError::Limit) {
        Ok(Some(rewrite(settings, code)))
    } else {
        Ok(Some(EditorialStep::Failed { code: code.into() }))
    }
}

#[cfg(test)]
mod tests;
