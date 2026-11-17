//! Supervisão da coleta, backpressure e confirmação durável de cada lote.

use anyhow::{Context, Result};
use chrono::Utc;
use std::{sync::Arc, time::Duration};
use stories_collectors::{
    Collector, SourceKind,
    http::{HttpClient, Validators},
};
use stories_config::{Settings, Source};
use stories_normalize::{NormalizedArticle, Normalizer};
use stories_store::{JobFailure, Store};
use tokio::{
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinSet,
};

struct NormalizationPool {
    engine: Normalizer,
    permits: Arc<Semaphore>,
}

struct PendingBatch {
    source: i32,
    articles: Vec<NormalizedArticle>,
    validators: Validators,
    not_modified: bool,
    next_poll: chrono::DateTime<Utc>,
    ack: oneshot::Sender<()>,
}

pub async fn run(settings: Settings, store: Store, mut stop: watch::Receiver<bool>) -> Result<()> {
    if !settings.collection.enabled {
        wait_for_stop(&mut stop).await;
        return Ok(());
    }
    let http = HttpClient::new(
        &settings.collection.user_agent,
        settings.collection.max_response_bytes,
        settings.collection.per_host,
    )?;
    run_with_http(settings, store, http, stop).await
}

pub async fn run_with_http(
    settings: Settings,
    store: Store,
    http: HttpClient,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    if !settings.collection.enabled {
        wait_for_stop(&mut stop).await;
        return Ok(());
    }
    http.ensure_news_egress()?;
    let collector = Collector::new(
        http,
        settings.collection.max_items,
        settings.pipeline.max_age_hours,
    )?;
    let normalizer = Arc::new(NormalizationPool {
        engine: Normalizer::new()?,
        permits: Arc::new(Semaphore::new(2)),
    });
    let (sender, mut receiver) = mpsc::channel::<PendingBatch>(settings.pipeline.channel_capacity);
    let mut producers = JoinSet::new();
    for source in settings.sources.iter().filter(|s| s.enabled) {
        let interval = i32::try_from(source.interval_s).context("intervalo de fonte inválido")?;
        let id = store
            .register_source(
                &source.name,
                &source.layer,
                &source.kind,
                &source.url,
                i16::from(source.weight),
                interval,
            )
            .await?;
        producers.spawn(poll_source(
            source.clone(),
            id,
            store.clone(),
            collector.clone(),
            normalizer.clone(),
            sender.clone(),
            stop.clone(),
        ));
    }
    drop(sender);
    let mut worker = tokio::spawn(cluster_worker(
        store.clone(),
        settings.clone(),
        stop.clone(),
    ));
    loop {
        tokio::select! {
            result=&mut worker=> {
                producers.abort_all();
                result.context("worker de cluster encerrou com panic")??;
                if !*stop.borrow(){anyhow::bail!("worker de cluster encerrou antes do shutdown");}
                break;
            }
            item=receiver.recv()=> {
                let Some(batch)=item else {
                    if !*stop.borrow(){
                        if settings.sources.iter().any(|s|s.enabled){anyhow::bail!("todos os coletores encerraram inesperadamente");}
                        wait_for_stop(&mut stop).await;
                    }
                    break;
                };
                let stats=store.ingest_batch(batch.source,&batch.articles,batch.validators.etag.as_deref(),batch.validators.last_modified.as_deref(),batch.not_modified,batch.next_poll).await?;
                tracing::info!(source_id=batch.source,inserted=stats.inserted,duplicates=stats.duplicates,"lote de coleta confirmado");
                let _=batch.ack.send(());
            }
            result=producers.join_next(), if !producers.is_empty()=> {
                match result {Some(Ok(Ok(()))) if *stop.borrow()=>{},Some(Ok(Err(error)))=>return Err(error),Some(Err(_))=>anyhow::bail!("coletor encerrou com panic"),_=>{}}
            }
        }
    }
    // Fechar a entrada libera produtores que ainda aguardavam canal ou confirmação.
    receiver.close();
    while let Some(batch) = receiver.recv().await {
        store
            .ingest_batch(
                batch.source,
                &batch.articles,
                batch.validators.etag.as_deref(),
                batch.validators.last_modified.as_deref(),
                batch.not_modified,
                batch.next_poll,
            )
            .await?;
        let _ = batch.ack.send(());
    }
    while let Some(result) = producers.join_next().await {
        result.context("coletor encerrou com panic")??;
    }
    if !worker.is_finished() {
        worker.await.context("worker encerrou com panic")??;
    }
    Ok(())
}

async fn poll_source(
    source: Source,
    id: i32,
    store: Store,
    collector: Collector,
    normalizer: Arc<NormalizationPool>,
    sender: mpsc::Sender<PendingBatch>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let kind = SourceKind::parse(&source.kind)?;
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        if store.paused().await? {
            if stopped_after(&mut stop, Duration::from_secs(1)).await {
                return Ok(());
            }
            continue;
        }
        let state = store.source_state(id).await?;
        if state.blocked {
            if stopped_after(&mut stop, Duration::from_secs(30)).await {
                return Ok(());
            }
            continue;
        }
        let wait = (state.next_poll_at - Utc::now())
            .to_std()
            .unwrap_or_default();
        if stopped_after(&mut stop, wait).await {
            return Ok(());
        }
        if store.paused().await? {
            continue;
        }
        let validators = Validators {
            etag: state.etag,
            last_modified: state.last_modified,
        };
        let captured = tokio::select! {changed=stop.changed()=>{let _=changed;return Ok(());},batch=collector.collect(kind,&source.url,&validators,Utc::now())=>batch};
        let batch = match captured {
            Ok(batch) => batch,
            Err(error) => {
                let delay =
                    source_backoff(source.interval_s, state.failures + 1, error.retry_after());
                store
                    .source_failed(
                        id,
                        error.code(),
                        Utc::now() + chrono::Duration::seconds(delay as i64),
                    )
                    .await?;
                tracing::warn!(
                    source_id = id,
                    code = error.code(),
                    retry_in_s = delay,
                    "fonte temporariamente isolada"
                );
                continue;
            }
        };
        let normalizer = normalizer.clone();
        let permit = normalizer
            .permits
            .clone()
            .acquire_owned()
            .await
            .context("pool CPU indisponível")?;
        let normalized = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            batch
                .articles
                .into_iter()
                .map(|a| normalizer.engine.normalize(a))
                .collect::<Result<Vec<_>, _>>()
        })
        .await
        .context("normalização isolada falhou")?;
        let articles = match normalized {
            Ok(articles) => articles,
            Err(_) => {
                store
                    .source_failed(
                        id,
                        "normalization",
                        Utc::now() + chrono::Duration::seconds(source.interval_s as i64),
                    )
                    .await?;
                continue;
            }
        };
        let (ack_sender, ack_receiver) = oneshot::channel();
        let jitter =
            (uuid::Uuid::new_v4().as_u128() % (u128::from(source.interval_s / 10) + 1)) as u64;
        let item = PendingBatch {
            source: id,
            articles,
            validators: batch.validators,
            not_modified: batch.not_modified,
            next_poll: Utc::now() + chrono::Duration::seconds((source.interval_s + jitter) as i64),
            ack: ack_sender,
        };
        tokio::select! {changed=stop.changed()=>{let _=changed;return Ok(());},result=sender.send(item)=>{if result.is_err(){if *stop.borrow(){return Ok(());}anyhow::bail!("ingestão encerrou antes do coletor");}}}
        tokio::select! {changed=stop.changed()=>{let _=changed;return Ok(());},result=ack_receiver=>{if result.is_err(){if *stop.borrow(){return Ok(());}anyhow::bail!("lote não foi confirmado");}}}
    }
}

async fn cluster_worker(
    store: Store,
    settings: Settings,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        if store.paused().await? {
            if stopped_after(&mut stop, Duration::from_secs(1)).await {
                return Ok(());
            }
            continue;
        }
        if let Some(job) = store
            .claim_kind(
                Duration::from_secs(settings.database.lease_s),
                settings.database.max_attempts,
                "cluster",
            )
            .await?
        {
            if let Err(error) = store
                .cluster_job(
                    &job,
                    settings.pipeline.cluster_similarity,
                    settings.pipeline.max_age_hours,
                )
                .await
            {
                let failure = match error {
                    stories_store::StoreError::LostLease => {
                        tracing::warn!(job_id = job.id, "lease de cluster perdido");
                        continue;
                    }
                    stories_store::StoreError::InvalidJob
                    | stories_store::StoreError::InvalidOptions => JobFailure::Permanent,
                    _ => JobFailure::Transient,
                };
                store
                    .retry(&job, failure, settings.database.max_attempts)
                    .await?;
                tracing::warn!(job_id = job.id, "cluster reagendado ou isolado");
            }
        } else if stopped_after(&mut stop, Duration::from_millis(250)).await {
            return Ok(());
        }
    }
}

pub fn source_backoff(interval: u64, failures: i32, retry_after: Option<u64>) -> u64 {
    let factor = 1u64 << u32::try_from(failures.saturating_sub(3).clamp(0, 8)).unwrap_or(0);
    interval
        .saturating_mul(factor)
        .min(3600)
        .max(interval)
        .max(retry_after.unwrap_or(0))
}

pub async fn wait_for_stop(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

async fn stopped_after(stop: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    if *stop.borrow() {
        return true;
    }
    tokio::select! {result=stop.changed()=>result.is_err()||*stop.borrow(),_=tokio::time::sleep(delay)=>false}
}

#[cfg(test)]
mod tests {
    #[test]
    fn circuit_breaker_waits_and_honors_rate_limit() {
        assert_eq!(super::source_backoff(60, 1, None), 60);
        assert_eq!(super::source_backoff(60, 4, None), 120);
        assert_eq!(super::source_backoff(60, 32, None), 3600);
        assert_eq!(super::source_backoff(60, 1, Some(600)), 600);
    }
}
