//! Executa exatamente um job contra o Graph de contrato; nunca integra a imagem de produção.
use super::*;
use stories_publisher::token::TokenClient;

pub async fn run_once() -> Result<()> {
    let url = std::env::var("STORIES_CHAOS_DATABASE").context("banco de teste ausente")?;
    let endpoint = std::env::var("STORIES_CHAOS_GRAPH").context("Graph de contrato ausente")?;
    let root = std::env::var("STORIES_CHAOS_ARTIFACTS").context("artefatos de teste ausentes")?;
    let store = Store::connect(&url, 4, Duration::from_secs(10)).await?;
    let mut settings = Settings::load(
        Path::new("config/exemplo.toml"),
        Path::new("config/sources.toml"),
    )?;
    settings.instagram.dry_run = false;
    settings.imaging.artifacts_dir = root;
    let http = HttpClient::shared("BreakingBytesChaos/0.1", 1024, 1)?.trusted_api_client();
    let graph = Graph::test_server(http.clone(), &format!("{endpoint}v27.0/"))?;
    let account = graph.account_key();
    let publisher = Publisher {
        settings,
        account,
        healthy: Arc::new(AtomicBool::new(true)),
        remote: Some(Remote {
            graph,
            r2: R2::new(
                http.clone(),
                &"a".repeat(32),
                "stories-img",
                "test",
                "test",
                Duration::from_secs(3600),
            )?,
            token: crate::token::TokenMonitor::test_facebook(TokenClient::test_server(
                http,
                Login::Facebook,
                &endpoint,
            )?)?,
        }),
    };
    let lease = Duration::from_secs(30);
    let job = if let Some(job) = store.claim_kind(lease, 5, "reconcile").await? {
        job
    } else {
        store
            .claim_kind(lease, 5, "publish")
            .await?
            .context("job de teste ausente")?
    };
    publisher.process(&store, &job).await?;
    store.close().await;
    Ok(())
}
