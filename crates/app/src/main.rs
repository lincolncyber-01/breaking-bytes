//! Inicialização sem exibir configurações, DSNs ou cadeias de erro externas.

use anyhow::{Context, Result};
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    path::Path,
    time::Duration,
};
use stories_app::{AppState, router};
use stories_config::Settings;
use stories_store::Store;
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, prelude::*};

fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_target(false)
                .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                    stories_app::token::log_target_allowed(metadata.target())
                })),
        )
        .try_init()
        .map_err(|_| anyhow::anyhow!("não foi possível inicializar os logs"))?;
    let config_path =
        std::env::var("STORIES_CONFIG").unwrap_or_else(|_| "config/exemplo.toml".into());
    let sources_path =
        std::env::var("STORIES_SOURCES").unwrap_or_else(|_| "config/sources.toml".into());
    let settings = Settings::load(Path::new(&config_path), Path::new(&sources_path))?;
    if std::env::args().any(|arg| arg == "--healthcheck") {
        return healthcheck(settings.admin.bind);
    }
    if let Some(position) = std::env::args().position(|arg| arg == "--render-example") {
        let args = std::env::args().collect::<Vec<_>>();
        let path = args
            .get(position + 1)
            .context("informe o caminho JPEG do exemplo")?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(stories_app::imaging::example(Path::new(path)));
    }
    if let Some(position) = std::env::args().position(|arg| arg == "--evaluate-editorial") {
        let args = std::env::args().collect::<Vec<_>>();
        let input = args
            .get(position + 1)
            .context("informe corpus JSON e caminho do relatório")?;
        let output = args
            .get(position + 2)
            .context("informe caminho do relatório")?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(stories_app::evaluation::run(
            &settings,
            Path::new(input),
            Path::new(output),
        ));
    }
    if std::env::args().any(|arg| arg == "--token-fingerprint") {
        let secret = stories_config::Secret::from_environment(&settings.instagram.access_token)?;
        let credential = stories_publisher::token::Credential::new(secret.expose())?;
        writeln!(
            &mut std::io::stdout().lock(),
            "{}",
            blake3::Hash::from_bytes(credential.fingerprint()).to_hex()
        )
        .context("não foi possível escrever o hash da credencial")?;
        return Ok(());
    }
    let secrets = settings.resolve_secrets()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("não foi possível criar o runtime")?;
    runtime.block_on(async move {
        let store = Store::connect(
            secrets.database_url.expose(),
            settings.database.max_connections,
            Duration::from_secs(settings.database.timeout_s),
        )
        .await?;
        if std::env::args().any(|arg| arg == "--migrate") {
            store.migrate().await?;
            tracing::info!("migrations aplicadas");
            store.close().await;
            return Ok(());
        }
        // Readiness exige schema instalado; migrations são um passo separado no deploy.
        store
            .paused()
            .await
            .context("schema indisponível; execute --migrate antes de iniciar")?;
        let listener = TcpListener::bind(settings.admin.bind)
            .await
            .context("não foi possível abrir a Admin API")?;
        let publishing_ready=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let token_state=std::sync::Arc::new(stories_app::token::TokenState::new(i64::from(settings.instagram.token.check_interval_s)*2+20,i64::from(settings.instagram.token.warning_days)));
        let publisher_admin=std::sync::Arc::new(stories_app::publication::AdminPublisher::default());
        let app = router(AppState {
            store: store.clone(),
            admin_token: secrets.admin_token.clone(),
            dry_run: settings.instagram.dry_run,
            collection_enabled: settings.collection.enabled,
            editorial_enabled: settings.llm.enabled,
            imaging_enabled: settings.imaging.enabled,
            publishing_enabled:settings.publishing.enabled,
            corrections_enabled:settings.corrections.enabled,
            publisher_admin:publisher_admin.clone(),
            publishing_ready:publishing_ready.clone(),
            token_state:token_state.clone(),
        });
        let (stop_sender,mut server_stop)=tokio::sync::watch::channel(false);
        let mut pipeline=tokio::spawn(stories_app::pipeline::run(settings.clone(),store.clone(),secrets,stories_app::publication::PublisherRuntime{healthy:publishing_ready,token_state,admin:publisher_admin},server_stop.clone()));
        let signal_sender=stop_sender.clone();
        let signal=tokio::spawn(async move{shutdown().await;let _=signal_sender.send(true);});
        tracing::info!(bind = %settings.admin.bind, dry_run = settings.instagram.dry_run,
            phase = 5, collection_enabled=settings.collection.enabled, editorial_enabled=settings.llm.enabled,"serviço iniciado; publicação controlada por modo e preflight");
        let server=axum::serve(listener, app).with_graceful_shutdown(async move {
            stories_app::collection::wait_for_stop(&mut server_stop).await;
        });
        let server=async move{server.await};
        tokio::pin!(server);
        let result=tokio::select! {
            result=&mut server=>{
                let _=stop_sender.send(true);
                result.context("servidor administrativo encerrou com falha")?;
                tokio::time::timeout(Duration::from_secs(30),&mut pipeline).await.context("shutdown excedeu 30 segundos")?
                    .context("supervisor encerrou com panic")?
            }
            result=&mut pipeline=>{
                let stopping=*stop_sender.borrow();
                let _=stop_sender.send(true);
                result.context("supervisor encerrou com panic")??;
                if !stopping{anyhow::bail!("pipeline encerrou antes do servidor");}
                tokio::time::timeout(Duration::from_secs(30),&mut server).await
                    .context("shutdown HTTP excedeu 30 segundos")?.context("servidor encerrou com falha")
            }
        };
        signal.abort();
        store.close().await;
        result
    })
}

/// Sonda local executada antes do runtime, inclusive na imagem sem shell ou curl.
fn healthcheck(mut address: SocketAddr) -> Result<()> {
    if address.ip().is_unspecified() {
        address.set_ip(match address.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    let timeout = Some(Duration::from_secs(2));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .context("health check não conseguiu conectar à API local")?;
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    stream
        .write_all(b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.take(512).read_to_string(&mut response)?;
    if !response.starts_with("HTTP/1.1 200 ") {
        anyhow::bail!("readiness local não está saudável");
    }
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if result.is_err() { tracing::error!("falha ao escutar SIGINT"); }
                    }
                    _ = signal.recv() => {}
                }
            }
            Err(_) => {
                tracing::error!("falha ao escutar SIGTERM");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("shutdown gracioso solicitado");
}
