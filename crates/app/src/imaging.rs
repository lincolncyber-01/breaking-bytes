//! Worker de imagem: download protegido, CPU serial e persistência atômica do JPEG.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use stories_collectors::http::{HttpClient, Validators};
use stories_config::Settings;
use stories_core::EventState;
use stories_imaging::{
    ImagingError,
    assets::Catalog,
    render::{Renderer, TEMPLATE},
    validate::{self, Limits},
};
use stories_store::{EditorialStep, Job, JobFailure, RenderArtifact, Store, StoreError};
use tokio::{io::AsyncWriteExt, sync::watch};

const ORIGINAL_CREDIT: &str = "Arte original: Breaking Bytes";

struct Processor {
    catalog: Catalog,
    renderer: Arc<Mutex<Renderer>>,
    http: HttpClient,
}

pub async fn run(
    settings: Settings,
    store: Store,
    http: HttpClient,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    if !settings.imaging.enabled {
        crate::collection::wait_for_stop(&mut stop).await;
        return Ok(());
    }
    let metadata = tokio::fs::metadata(&settings.imaging.manifest)
        .await
        .context("manifesto de imagens indisponível")?;
    if metadata.len() > 1_048_576 {
        anyhow::bail!("manifesto de imagens excede 1 MiB");
    }
    let bytes = tokio::fs::read(&settings.imaging.manifest).await?;
    let domains = settings.imaging.allowed_domains.clone();
    let (catalog, renderer) = tokio::task::spawn_blocking(move || {
        Ok::<_, ImagingError>((Catalog::parse(&bytes, domains)?, Renderer::new()?))
    })
    .await??;
    let processor = Processor {
        catalog,
        renderer: Arc::new(Mutex::new(renderer)),
        http,
    };
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        let lease = Duration::from_secs(settings.database.lease_s);
        let Some(job) = store
            .claim_kind(lease, settings.database.max_attempts, "render")
            .await?
        else {
            tokio::select! {_=stop.changed()=>{},_=tokio::time::sleep(Duration::from_millis(250))=>{}}
            continue;
        };
        let Some(result) = crate::editorial::owned_work(
            &store,
            &job,
            lease,
            &mut stop,
            processor.process(&settings, &store, &job),
        )
        .await
        else {
            let _ = store
                .retry(&job, JobFailure::Transient, settings.database.max_attempts)
                .await;
            return Ok(());
        };
        if let Err(error) = result {
            if matches!(
                error.downcast_ref::<StoreError>(),
                Some(StoreError::LostLease)
            ) {
                tracing::warn!(
                    job_id = job.id,
                    "lease de render perdido; resultado descartado"
                );
                continue;
            }
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
            tracing::warn!(job_id = job.id, "render reagendado ou isolado");
        }
    }
}

impl Processor {
    async fn process(&self, settings: &Settings, store: &Store, job: &Job) -> Result<()> {
        let event = job
            .payload
            .get("event_id")
            .and_then(Value::as_i64)
            .filter(|v| *v > 0)
            .ok_or(StoreError::InvalidJob)?;
        let revision = job
            .payload
            .get("revision")
            .and_then(Value::as_i64)
            .filter(|v| *v >= 0)
            .ok_or(StoreError::InvalidJob)?;
        let context = store.editorial_context(event).await?;
        if context
            .verified_revision
            .is_some_and(|v| v != context.revision)
        {
            store
                .editorial_commit(job, &context, EditorialStep::Skip)
                .await?;
            return Ok(());
        }
        if context.state != EventState::Checked || context.revision != revision {
            store.complete(job).await?;
            return Ok(());
        }
        if store.event_blocked(event).await? {
            store
                .editorial_commit(
                    job,
                    &context,
                    EditorialStep::Rejected {
                        code: "policy_blocked".into(),
                    },
                )
                .await?;
            return Ok(());
        }
        let draft = context.draft.clone().ok_or(StoreError::InvalidJob)?;
        let assessment = context.assessment.as_ref().ok_or(StoreError::InvalidJob)?;
        let primary = assessment.sources.first().ok_or(StoreError::InvalidJob)?;
        let now = chrono::Utc::now();
        let cutoff = now - chrono::Duration::hours(i64::from(settings.pipeline.max_age_hours));
        let fresh = if context.review.is_some() {
            store
                .revision_fresh(
                    event,
                    settings.pipeline.max_age_hours,
                    settings.instagram.dry_run,
                )
                .await?
        } else {
            !assessment
                .sources
                .iter()
                .any(|s| !s.date_confirmed || s.published_at.is_none_or(|d| d < cutoff || d > now))
        };
        if !fresh {
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
        let og = store.primary_image(event).await?;
        let candidates =
            self.catalog
                .candidates(&primary.article_url, &draft.entities, og.as_deref(), now);
        let mut selected = None;
        let limits = Limits {
            max_bytes: settings.imaging.max_bytes,
            max_pixels: settings.imaging.max_pixels,
        };
        // No máximo quatro tentativas de rede; sem material elegível há arte original.
        for asset in candidates.into_iter().take(4) {
            let response = match self
                .http
                .get_bounded(&asset.url, &Validators::default(), limits.max_bytes)
                .await
            {
                Ok(response) if response.status == 200 => response,
                _ => {
                    tracing::warn!(
                        event_id = event,
                        "imagem licenciada indisponível nesta tentativa"
                    );
                    continue;
                }
            };
            let check = asset.clone();
            let decoded = tokio::task::spawn_blocking(move || {
                check.verify_bytes(&response.body)?;
                validate::decode(
                    &response.body,
                    response.content_type.as_deref().unwrap_or(""),
                    limits,
                )
            })
            .await?;
            match decoded {
                Ok(image) => {
                    selected = Some((image, asset));
                    break;
                }
                Err(error) => tracing::warn!(
                    event_id = event,
                    code = error.code(),
                    "imagem descartada pela validação"
                ),
            }
        }
        let renderer = self.renderer.clone();
        let result=tokio::task::spawn_blocking(move||{
            let mut renderer=renderer.lock().map_err(|_|ImagingError::Render)?;
            let original=json!({"kind":"original","generator":TEMPLATE,"credit":ORIGINAL_CREDIT,"license":"owned"});
            let (rendered,provenance)=if let Some((image,asset))=selected {
                match renderer.render(&draft,Some(&image),&asset.credit) {
                    Ok(rendered)=>(rendered,json!({"kind":"licensed","asset":asset})),
                    Err(ImagingError::NeedsRewrite|ImagingError::MissingGlyph)=>(renderer.render(&draft,None,ORIGINAL_CREDIT)?,original),
                    Err(error)=>return Err(error),
                }
            }else{(renderer.render(&draft,None,ORIGINAL_CREDIT)?,original)};
            let digest=*blake3::hash(&rendered.jpeg).as_bytes();
            Ok::<_,ImagingError>((rendered.jpeg,rendered.layout,provenance,digest))
        }).await?;
        match result {
            Ok((bytes, layout, provenance, digest)) => {
                let path = tokio::time::timeout(
                    Duration::from_secs(10),
                    persist(
                        Path::new(&settings.imaging.artifacts_dir),
                        event,
                        revision,
                        &bytes,
                        digest,
                    ),
                )
                .await
                .context("persistência do JPEG excedeu o timeout")??;
                let artifact = RenderArtifact {
                    path,
                    hash: digest,
                    bytes: bytes.len(),
                    layout,
                    provenance,
                    dry_run: settings.instagram.dry_run,
                };
                let disposition = store
                    .render_commit(
                        job,
                        &context,
                        Some(artifact),
                        settings.pipeline.max_rewrite_attempts,
                        "",
                    )
                    .await?;
                tracing::info!(
                    event_id = event,
                    revision,
                    ?disposition,
                    "trabalho de render confirmado"
                );
            }
            Err(error @ (ImagingError::NeedsRewrite | ImagingError::MissingGlyph)) => {
                store
                    .render_commit(
                        job,
                        &context,
                        None,
                        settings.pipeline.max_rewrite_attempts,
                        error.code(),
                    )
                    .await?;
                tracing::warn!(
                    event_id = event,
                    code = error.code(),
                    "texto precisa de redação mais curta para o template"
                );
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

async fn persist(
    root: &Path,
    event: i64,
    revision: i64,
    bytes: &[u8],
    digest: [u8; 32],
) -> Result<String> {
    let relative = PathBuf::from(event.to_string())
        .join(revision.to_string())
        .join(format!("{}.jpg", blake3::Hash::from(digest).to_hex()));
    let parent = root.join(event.to_string()).join(revision.to_string());
    tokio::fs::create_dir_all(&parent).await?;
    let temporary = parent.join(format!("{}.part", uuid::Uuid::new_v4()));
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temporary, root.join(&relative)).await?;
    tokio::fs::File::open(&parent).await?.sync_all().await?;
    relative
        .to_str()
        .map(str::to_owned)
        .context("caminho de artefato inválido")
}

/// Exemplo offline: não abre Postgres nem exige credenciais externas.
pub async fn example(output: &Path) -> Result<()> {
    let rendered = tokio::task::spawn_blocking(|| {
        let mut renderer = Renderer::new()?;
        renderer.render(
            &stories_imaging::render::example_draft(),
            None,
            ORIGINAL_CREDIT,
        )
    })
    .await??;
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(output, &rendered.jpeg).await?;
    tokio::fs::write(
        output.with_extension("json"),
        serde_json::to_vec_pretty(&rendered.layout)?,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
