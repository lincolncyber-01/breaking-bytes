//! Transporte compartilhado e supervisão dos estágios habilitados.

use anyhow::{Context, Result};
use std::time::Duration;
use stories_collectors::http::HttpClient;
use stories_config::{Secret, Settings};
use stories_editorial::client::AnthropicClient;
use stories_store::Store;
use tokio::{sync::watch, task::JoinSet};

pub async fn run(
    settings: Settings,
    store: Store,
    key: Option<Secret>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    if !settings.collection.enabled && !settings.llm.enabled && !settings.imaging.enabled {
        crate::collection::wait_for_stop(&mut stop).await;
        return Ok(());
    }
    let http = HttpClient::shared(
        &settings.collection.user_agent,
        settings.collection.max_response_bytes,
        settings.collection.per_host,
    )?;
    // A API fixa do LLM pode usar proxy; dados jornalísticos exigem DNS verificável.
    if settings.collection.enabled || settings.llm.enabled {
        http.ensure_news_egress()?;
    }
    for source in settings.sources.iter().filter(|s| s.enabled) {
        store
            .register_source(
                &source.name,
                &source.layer,
                &source.kind,
                &source.url,
                i16::from(source.weight),
                i32::try_from(source.interval_s)?,
            )
            .await?;
    }
    let origins = settings
        .sources
        .iter()
        .map(|s| (s.name.clone(), s.origin_group.clone(), s.enabled))
        .collect::<Vec<_>>();
    store.synchronize_sources(&origins).await?;
    let mut tasks = JoinSet::new();
    tasks.spawn(crate::collection::run_with_http(
        settings.clone(),
        store.clone(),
        http.clone(),
        stop.clone(),
    ));
    if settings.imaging.enabled {
        tasks.spawn(crate::imaging::run(
            settings.clone(),
            store.clone(),
            http.clone(),
            stop.clone(),
        ));
    }
    if settings.llm.enabled {
        let key = key.context("LLM habilitado exige chave no ambiente")?;
        let client = AnthropicClient::new(
            http.trusted_api_client(),
            key.expose(),
            &settings.llm.model,
            Duration::from_secs(settings.llm.timeout_s),
        )?;
        tasks.spawn(crate::editorial::run(
            settings,
            store,
            http,
            client,
            stop.clone(),
        ));
    }
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) if *stop.borrow() => {}
            Ok(Ok(())) => anyhow::bail!("estágio encerrou antes do shutdown"),
            Ok(Err(error)) => return Err(error),
            Err(_) => anyhow::bail!("estágio encerrou com panic"),
        }
    }
    Ok(())
}
