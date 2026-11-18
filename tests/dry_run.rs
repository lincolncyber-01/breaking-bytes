//! Contrato integrado: RSS até o JPEG e a intenção simulada, com Postgres real.
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use stories_config::Settings;
use stories_editorial::{
    Editorial, Task,
    client::{CompletionClient, LlmError},
};
use stories_normalize::Normalizer;
use stories_store::Store;
use stories_verify::Evidence;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

struct Confirmed;
impl crate::editorial::EvidenceClient for Confirmed {
    async fn hydrate(&self, mut sources: Vec<Evidence>) -> Vec<Evidence> {
        for source in &mut sources {
            source.date_confirmed = true;
        }
        sources
    }
}
struct EditorialContract;
impl CompletionClient for EditorialContract {
    async fn complete(&self, task: Task, data: Value) -> Result<Value, LlmError> {
        match task {
            Task::Draft => Ok(json!({
                "title":"Rust lança versão 1.100",
                "summary":["Rust lança versão 1.100.","A versão inclui correções."],
                "source":"Rust Blog","kind":"new","entities":["Rust"],"language":"pt-BR"
            })),
            Task::Check => {
                let article = &data["evidence"]["sources"][0]["article_id"];
                let segments = data["segments"]
                    .as_array()
                    .ok_or(LlmError::Input)?
                    .iter()
                    .enumerate()
                    .map(|(index, text)| {
                        json!({"index":index,"text":text,"supported":true,
                        "claims":[{"text":text,"supported":true,"evidence":[{"article_id":article,
                            "quote":"Rust lança versão 1.100. A versão inclui correções."}]}]})
                    })
                    .collect::<Vec<_>>();
                Ok(json!({"language_ok":true,"neutral":true,"segments":segments}))
            }
            Task::ReviewChange | Task::CheckRevision => Err(LlmError::Input),
        }
    }
}

#[tokio::test]
async fn rss_to_dry_run_is_durable_replayable_and_has_no_remote_identity() -> Result<()> {
    let image =
        std::env::var("STORIES_TEST_POSTGRES_IMAGE").unwrap_or_else(|_| "pgvector/pgvector".into());
    let container = GenericImage::new(image.as_str(), "0.8.2-pg16")
        .with_exposed_port(5432.tcp())
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "integration-only")
        .start()
        .await?;
    let database = format!(
        "postgres://postgres:integration-only@{}:{}/postgres",
        container.get_host().await?,
        container.get_host_port_ipv4(5432).await?
    );
    let store = Store::connect(&database, 5, Duration::from_secs(10)).await?;
    store.migrate().await?;
    let source = store
        .register_source(
            "Rust Blog",
            "A",
            "rss",
            "https://blog.rust-lang.org/feed.xml",
            5,
            60,
        )
        .await?;
    let published = (Utc::now() - chrono::Duration::minutes(1)).to_rfc2822();
    let feed = format!(
        "<rss version=\"2.0\"><channel><title>Rust Blog</title><link>https://blog.rust-lang.org</link><description>Comunicados</description><item><title>Rust lança versão 1.100</title><link>https://blog.rust-lang.org/announcement?utm_source=contract</link><pubDate>{published}</pubDate><description>Rust lança versão 1.100. A versão inclui correções.</description></item></channel></rss>"
    );
    let normalizer = Normalizer::new()?;
    let articles = stories_collectors::parsers::feed(feed.as_bytes(), 200)?
        .into_iter()
        .map(|article| normalizer.normalize(article))
        .collect::<Result<Vec<_>, _>>()?;
    let ingested = store
        .ingest_batch(
            source,
            &articles,
            Some("contract-v1"),
            None,
            false,
            Utc::now(),
        )
        .await?;
    assert_eq!(ingested.inserted, 1);
    let cluster = store
        .claim_kind(Duration::from_secs(120), 5, "cluster")
        .await?
        .context("outbox de cluster ausente")?;
    let event = store.cluster_job(&cluster, 0.86, 6).await?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut settings = Settings::load(
        &root.join("config/exemplo.toml"),
        &root.join("config/sources.toml"),
    )?;
    let artifacts = std::env::temp_dir().join(format!("stories-e2e-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&artifacts).await?;
    settings.imaging.artifacts_dir = artifacts.to_string_lossy().into_owned();
    assert!(settings.instagram.dry_run);
    let verify = store
        .claim_kind(Duration::from_secs(120), 5, "verify")
        .await?
        .context("outbox editorial ausente")?;
    crate::editorial::process(
        &settings,
        &store,
        &verify,
        &EditorialContract,
        &Confirmed,
        Arc::new(Editorial::new()?),
    )
    .await?;
    crate::imaging::tests::render_fixture(&settings, &store, event).await?;
    let publication = crate::publication::tests::dry_fixture(&settings, &store, event).await?;
    let record = store.publication(publication).await?;
    assert_eq!(record.status, "dry_run");
    assert!(
        record.media_id.is_none() && record.container_id.is_none() && record.image_key.is_none()
    );
    let artifact: String =
        sqlx::query_scalar("SELECT artifact_path FROM rendered_stories WHERE event_id=$1")
            .bind(event)
            .fetch_one(store.pool())
            .await?;
    let jpeg = tokio::fs::read(artifacts.join(artifact)).await?;
    assert!(jpeg.starts_with(&[0xff, 0xd8]) && jpeg.len() > 10_000);
    let replay = store
        .ingest_batch(
            source,
            &articles,
            Some("contract-v1"),
            None,
            false,
            Utc::now(),
        )
        .await?;
    assert_eq!((replay.inserted, replay.duplicates), (0, 1));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM publications")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(count, 1);
    assert!(
        store
            .claim_kind(Duration::from_secs(120), 5, "publish")
            .await?
            .is_none()
    );
    store.close().await;
    tokio::fs::remove_dir_all(artifacts).await?;
    Ok(())
}
