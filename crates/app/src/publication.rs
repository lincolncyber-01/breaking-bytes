//! Worker de publicação: simulação integral, efeito remoto único e reconciliação conservadora.

use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    path::{Component, Path},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use stories_collectors::http::HttpClient;
use stories_config::{Secrets, Settings};
use stories_publisher::{
    graph::{ContainerStatus, Graph, Login},
    r2::R2,
};
use stories_scheduler::{Policy, Signals};
use stories_store::{Job, JobFailure, PublicationPolicy, PublicationStep, Store, StoreError};
use tokio::{io::AsyncReadExt, sync::watch};

struct Remote {
    graph: Graph,
    r2: R2,
}
struct Publisher {
    settings: Settings,
    account: String,
    remote: Option<Remote>,
    healthy: Arc<AtomicBool>,
}

pub async fn run(
    settings: Settings,
    store: Store,
    http: HttpClient,
    secrets: Secrets,
    healthy: Arc<AtomicBool>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let remote = if settings.instagram.dry_run {
        None
    } else {
        let login = match settings.instagram.login {
            stories_config::Login::Facebook => Login::Facebook,
            stories_config::Login::Instagram => Login::Instagram,
        };
        let required = |secret: Option<stories_config::Secret>| {
            secret.context("credencial de publicação ausente")
        };
        Some(Remote {
            graph: Graph::new(
                http.trusted_api_client(),
                login,
                &settings.instagram.api_version,
                required(secrets.instagram_user_id)?.expose(),
                required(secrets.instagram_token)?.expose(),
            )?,
            r2: R2::new(
                http.trusted_api_client(),
                required(secrets.r2_account)?.expose(),
                &settings.storage.bucket,
                required(secrets.r2_access)?.expose(),
                required(secrets.r2_secret)?.expose(),
                Duration::from_secs(u64::from(settings.storage.ttl_minutes) * 60),
            )?,
        })
    };
    let account = remote
        .as_ref()
        .map(|v| v.graph.account_key())
        .unwrap_or_else(|| "dry_run-local".into());
    let publisher = Publisher {
        settings,
        account,
        remote,
        healthy,
    };
    let mut last_maintenance = tokio::time::Instant::now() - Duration::from_secs(60);
    loop {
        if *stop.borrow() {
            publisher.healthy.store(false, Ordering::Relaxed);
            return Ok(());
        }
        if last_maintenance.elapsed() >= Duration::from_secs(60) {
            let maintenance = async {
                store
                    .recover_publications(&publisher.account, publisher.settings.instagram.dry_run)
                    .await?;
                let ready =
                    tokio::time::timeout(Duration::from_secs(20), publisher.preflight(&store))
                        .await
                        .is_ok_and(|r| r.is_ok());
                publisher.healthy.store(ready, Ordering::Relaxed);
                if !ready {
                    tracing::warn!("preflight de publicação falhou; novos envios suspensos");
                }
                match tokio::time::timeout(Duration::from_secs(20), publisher.cleanup(&store)).await
                {
                    Ok(result) => result?,
                    Err(_) => tracing::warn!("limpeza R2 excedeu o ciclo; será retomada"),
                }
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! {
                _=crate::collection::wait_for_stop(&mut stop)=>{
                    publisher.healthy.store(false,Ordering::Relaxed);
                    return Ok(());
                }
                result=maintenance=>result?,
            }
            last_maintenance = tokio::time::Instant::now();
        }
        let lease = Duration::from_secs(publisher.settings.database.lease_s);
        // Reconciliação precede novos envios, inclusive durante pausa global.
        let mut claimed = None;
        for kind in ["reconcile", "publish", "schedule"] {
            if kind != "reconcile" && !publisher.healthy.load(Ordering::Relaxed) {
                continue;
            }
            claimed = store
                .claim_kind(lease, publisher.settings.database.max_attempts, kind)
                .await?;
            if claimed.is_some() {
                break;
            }
        }
        let Some(job) = claimed else {
            tokio::select! {_=stop.changed()=>{},_=tokio::time::sleep(Duration::from_millis(250))=>{}}
            continue;
        };
        let work = publisher.process(&store, &job);
        let Some(result) = crate::editorial::owned_work(&store, &job, lease, &mut stop, work).await
        else {
            // Se a barreira foi gravada, o kind persistido já é reconcile.
            let _ = store.defer_publication(&job, 5).await;
            publisher.healthy.store(false, Ordering::Relaxed);
            return Ok(());
        };
        if let Err(error) = result {
            if matches!(
                error.downcast_ref::<StoreError>(),
                Some(StoreError::LostLease)
            ) {
                continue;
            }
            tracing::warn!(
                job_id = job.id,
                "publicação pendente; detalhes externos omitidos"
            );
            let publication =
                if let Some(id) = job.payload.get("publication_id").and_then(Value::as_i64) {
                    Some(store.publication(id).await?)
                } else {
                    None
                };
            let retry = if publication
                .as_ref()
                .is_some_and(|p| p.request_started_at.is_some())
            {
                store.defer_publication(&job, 60).await
            } else if let Some(p) = publication
                .as_ref()
                .filter(|_| job.attempts >= publisher.settings.database.max_attempts)
            {
                store
                    .publication_step(&job, p.id, PublicationStep::Failed, publisher.policy(0, 80))
                    .await
                    .map(|_| ())
            } else {
                store
                    .retry(
                        &job,
                        JobFailure::Transient,
                        publisher.settings.database.max_attempts,
                    )
                    .await
            };
            match retry {
                Ok(()) | Err(StoreError::LostLease) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Publisher {
    async fn options(&self) -> Result<PublicationPolicy<'_>> {
        let (remote_usage, remote_limit) = if let Some(remote) = &self.remote {
            let q = remote.graph.quota().await?;
            (q.usage, q.total)
        } else {
            (0, 80)
        };
        Ok(self.policy(remote_usage, remote_limit))
    }
    fn policy(&self, remote_usage: u32, remote_limit: u32) -> PublicationPolicy<'_> {
        PublicationPolicy {
            account: &self.account,
            dry_run: self.settings.instagram.dry_run,
            policy: Policy {
                daily_cap: self.settings.pipeline.daily_cap,
                reserved: self
                    .settings
                    .publishing
                    .reserved_slots
                    .min(self.settings.pipeline.daily_cap - 1),
                breaking_score: self.settings.pipeline.breaking_score,
                min_gap_seconds: self.settings.pipeline.min_gap_seconds,
            },
            remote_usage,
            remote_limit,
            max_age_hours: self.settings.pipeline.max_age_hours,
        }
    }
    async fn preflight(&self, _store: &Store) -> Result<()> {
        if let Some(remote) = &self.remote {
            remote.graph.quota().await?;
            remote.graph.stories().await?;
            // Uma sonda reversível verifica assinatura, permissão de escrita, leitura e remoção.
            let bytes = vec![0xff, 0xd8, 0xff, 0xd9];
            let hash = *blake3::hash(&bytes).as_bytes();
            let key = format!("health/{}.jpg", uuid::Uuid::new_v4());
            remote.r2.put_verified(&key, bytes, hash).await?;
            remote.r2.delete(&key).await?;
        }
        Ok(())
    }
    async fn cleanup(&self, store: &Store) -> Result<()> {
        if let Some(remote) = &self.remote {
            for p in store
                .cleanup_candidates(&self.account, self.settings.storage.ttl_minutes as i32)
                .await?
            {
                if let Some(key) = p.image_key {
                    if remote.r2.delete(&key).await.is_ok() {
                        store.publication_cleaned(p.id).await?;
                    } else {
                        tracing::warn!(publication_id = p.id, "objeto R2 aguarda limpeza");
                    }
                }
            }
        }
        Ok(())
    }
    async fn process(&self, store: &Store, job: &Job) -> Result<()> {
        if job.kind == "schedule" {
            let event = positive(&job.payload, "event_id")?;
            let revision = job
                .payload
                .get("revision")
                .and_then(Value::as_i64)
                .filter(|v| *v >= 0)
                .context("revisão inválida")?;
            let context = store.editorial_context(event).await?;
            // Sem classificador de impacto calibrado, sinais conservadores explícitos e auditáveis.
            let reliability = match context.assessment.as_ref().map(|a| a.level) {
                Some(stories_verify::Level::Primary) => 1.0,
                Some(stories_verify::Level::Independent) => 0.8,
                _ => 0.5,
            };
            let signals = Signals {
                impact: 0.5,
                novelty: if context.kind == "new" { 1.0 } else { 0.5 },
                reliability,
                reach: 0.5,
                urgency: 0.0,
            };
            return Ok(store
                .schedule_publication(
                    job,
                    event,
                    revision,
                    signals.score()?,
                    serde_json::to_value(signals)?,
                    self.options().await?,
                )
                .await?);
        }
        let id = positive(&job.payload, "publication_id")?;
        let p = store.publication(id).await?;
        if p.account_key.as_deref() != Some(&self.account)
            || p.dry_run != self.settings.instagram.dry_run
        {
            anyhow::bail!("publicação pertence a outra conta ou modo");
        }
        if matches!(
            p.status.as_str(),
            "published" | "published_unresolved" | "dry_run" | "failed"
        ) {
            store.complete(job).await?;
            return Ok(());
        }
        if p.request_started_at.is_some() {
            let remote = self
                .remote
                .as_ref()
                .context("reconciliação exige modo real")?;
            let container = p
                .container_id
                .as_deref()
                .context("container ausente após barreira")?;
            // A lista é consultada, mas nem um único ID novo constitui prova de identidade.
            let stories = remote.graph.stories().await?;
            tracing::info!(
                publication_id = id,
                active_stories = stories.len(),
                "reconciliação consultou Stories ativos"
            );
            let options = self.policy(0, 80);
            if remote.graph.status(container).await? == ContainerStatus::Published {
                store
                    .publication_step(job, id, PublicationStep::ConfirmedWithoutId, options)
                    .await?;
            } else {
                store
                    .publication_step(job, id, PublicationStep::Uncertain, options)
                    .await?;
            }
            return Ok(());
        }
        let revision = p.revision.context("revisão da publicação ausente")?;
        let artifact = store.publication_artifact(p.event_id, revision).await?;
        let bytes = read_artifact(
            Path::new(&self.settings.imaging.artifacts_dir),
            &artifact.artifact_path,
            &artifact.content_hash,
            artifact.byte_length,
        )
        .await?;
        if p.artifact_hash.as_deref() != Some(artifact.content_hash.as_slice())
            || artifact.dry_run != p.dry_run
        {
            anyhow::bail!("artefato não corresponde à reserva");
        }
        if p.dry_run {
            store
                .publication_step(job, id, PublicationStep::Simulated, self.options().await?)
                .await?;
            return Ok(());
        }
        let remote = self.remote.as_ref().context("cliente externo ausente")?;
        if p.container_id.is_none() {
            let key = format!(
                "stories/{}/{id}/{}.jpg",
                self.account,
                blake3::Hash::from_bytes(
                    artifact
                        .content_hash
                        .as_slice()
                        .try_into()
                        .context("hash inválido")?
                )
                .to_hex()
            );
            // A chave é persistida antes do PUT para permitir limpeza mesmo após queda ou revisão nova.
            if !store
                .publication_step(
                    job,
                    id,
                    PublicationStep::UploadPlanned(&key),
                    self.options().await?,
                )
                .await?
            {
                return Ok(());
            }
            // PUT repetido sobrescreve a mesma chave com os mesmos bytes; não cria Story.
            remote
                .r2
                .put_verified(
                    &key,
                    bytes,
                    artifact
                        .content_hash
                        .as_slice()
                        .try_into()
                        .context("hash inválido")?,
                )
                .await?;
            if !store
                .publication_step(
                    job,
                    id,
                    PublicationStep::UploadPlanned(&key),
                    self.options().await?,
                )
                .await?
            {
                return Ok(());
            }
            let url = remote.r2.get_url(&key).await?;
            let container = remote.graph.create(url.expose()).await?;
            if !store
                .publication_step(
                    job,
                    id,
                    PublicationStep::Container(&container),
                    self.options().await?,
                )
                .await?
            {
                return Ok(());
            }
            store.defer_publication(job, 60).await?;
            return Ok(());
        }
        let container = p.container_id.as_deref().context("container ausente")?;
        match remote.graph.status(container).await? {
            ContainerStatus::InProgress => {
                if chrono::Utc::now() - p.created_at > chrono::Duration::minutes(10) {
                    store
                        .publication_step(job, id, PublicationStep::Failed, self.options().await?)
                        .await?;
                } else {
                    store.defer_publication(job, 60).await?;
                }
            }
            ContainerStatus::Error | ContainerStatus::Expired => {
                store
                    .publication_step(job, id, PublicationStep::Failed, self.options().await?)
                    .await?;
            }
            ContainerStatus::Published => {
                anyhow::bail!("container publicado sem barreira local; intervenção necessária");
            }
            ContainerStatus::Finished => {
                let options = self.options().await?;
                if !store
                    .publication_step(job, id, PublicationStep::Ready, options)
                    .await?
                {
                    return Ok(());
                }
                if !store
                    .publication_step(job, id, PublicationStep::Begin, self.options().await?)
                    .await?
                {
                    return Ok(());
                }
                // O futuro pode ser cancelado daqui em diante: a intenção já impede novo POST.
                match remote.graph.publish(container).await {
                    Ok(media) => {
                        store
                            .publication_step(job, id, PublicationStep::Published(&media), options)
                            .await?;
                    }
                    Err(_) => {
                        store
                            .publication_step(job, id, PublicationStep::Uncertain, options)
                            .await?;
                    }
                }
            }
        }
        Ok(())
    }
}
fn positive(value: &Value, key: &str) -> Result<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .filter(|v| *v > 0)
        .context("payload de publicação inválido")
}
async fn read_artifact(root: &Path, relative: &str, hash: &[u8], length: i64) -> Result<Vec<u8>> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|v| !matches!(v, Component::Normal(_)))
        || relative.as_os_str().is_empty()
    {
        anyhow::bail!("caminho do JPEG inválido");
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        let root = tokio::fs::canonicalize(root).await?;
        let path = tokio::fs::canonicalize(root.join(relative)).await?;
        if !path.starts_with(&root) {
            anyhow::bail!("JPEG fora do diretório de artefatos");
        }
        let file = tokio::fs::File::open(path).await?;
        if !file.metadata().await?.is_file() || !(1..=8 * 1024 * 1024).contains(&length) {
            anyhow::bail!("tamanho do JPEG inválido");
        }
        let mut bytes = Vec::new();
        file.take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() as i64 != length {
            anyhow::bail!("tamanho do JPEG difere da reserva");
        }
        let (bytes, digest) = tokio::task::spawn_blocking(move || {
            let digest = *blake3::hash(&bytes).as_bytes();
            (bytes, digest)
        })
        .await?;
        if digest != hash {
            anyhow::bail!("hash ou tamanho do JPEG difere da reserva");
        }
        Ok(bytes)
    })
    .await
    .context("leitura do JPEG excedeu o timeout")?
}

#[cfg(test)]
mod tests;
