use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::time::Duration;
use stories_app::{AppState, router};
use stories_config::Settings;
use stories_store::Store;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tower::ServiceExt;

#[tokio::test]
async fn health_auth_pause_and_fail_closed_readiness() -> Result<(), Box<dyn std::error::Error>> {
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
    let url = format!(
        "postgres://postgres:integration-only@{}:{}/postgres",
        container.get_host().await?,
        container.get_host_port_ipv4(5432).await?
    );
    let store = Store::connect(&url, 10, Duration::from_secs(10)).await?;
    store.migrate().await?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = Settings::load(
        &root.join("config/exemplo.toml"),
        &root.join("config/sources.toml"),
    )?;
    let token = "test-admin-token-com-mais-de-32-bytes";
    #[cfg(unix)]
    exercise_binary(&url, token, &root).await?;
    let secrets = config.resolve_with(|name| match name {
        "DATABASE_URL" => Some(url.clone()),
        "ADMIN_TOKEN" => Some(token.into()),
        _ => None,
    })?;
    let state = AppState {
        token_state: std::sync::Arc::new(stories_app::token::TokenState::default()),
        publishing_enabled: true,
        corrections_enabled: false,
        publisher_admin: std::sync::Arc::new(stories_app::publication::AdminPublisher::default()),
        publishing_ready: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        editorial_enabled: false,
        imaging_enabled: false,
        store: store.clone(),
        admin_token: secrets.admin_token,
        dry_run: true,
        collection_enabled: false,
    };
    let app = router(state.clone());
    for path in ["/health/live", "/health/ready"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
    }
    for value in [None, Some("Bearer errado"), Some("Basic dGVzdA==")] {
        let mut request = Request::builder().uri("/admin/status");
        if let Some(value) = value {
            request = request.header("authorization", value);
        }
        assert_eq!(
            app.clone()
                .oneshot(request.body(Body::empty())?)
                .await?
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let request = Request::builder()
        .method("PUT")
        .uri("/admin/pause")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(r#"{"paused":true}"#))?;
    assert_eq!(app.clone().oneshot(request).await?.status(), StatusCode::OK);
    assert!(store.paused().await?);
    let request = Request::builder()
        .method("PUT")
        .uri("/admin/pause")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{"paused":true,"extra":"{}"}}"#,
            "x".repeat(2000)
        )))?;
    assert_eq!(
        app.clone().oneshot(request).await?.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut live = state;
    live.dry_run = false;
    let response = router(live)
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    store.close().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[cfg(unix)]
async fn exercise_binary(
    url: &str,
    token: &str,
    root: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::process::Stdio;
    use tokio::{
        net::TcpListener,
        process::Command,
        time::{sleep, timeout},
    };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    drop(listener);
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stories-tech"));
        command
            .env("DATABASE_URL", url)
            .env("ADMIN_TOKEN", token)
            .env("RUST_LOG", "info,sqlx=warn")
            .env("STORIES__COLLECTION__ENABLED", "false")
            .env("STORIES_CONFIG", root.join("config/exemplo.toml"))
            .env("STORIES_SOURCES", root.join("config/sources.toml"))
            .env("STORIES__ADMIN__BIND", address.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    };
    let migrated = timeout(Duration::from_secs(15), command().arg("--migrate").output()).await??;
    assert!(migrated.status.success(), "binário deve aplicar migrations");
    let child = command().spawn()?;
    let pid = child.id().ok_or("processo sem identificador")?;
    timeout(Duration::from_secs(15), async {
        loop {
            let probe = command().arg("--healthcheck").output().await?;
            if probe.status.success() {
                return Ok::<(), std::io::Error>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await??;
    let killed = timeout(
        Duration::from_secs(2),
        Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status(),
    )
    .await??;
    assert!(killed.success());
    let output = timeout(Duration::from_secs(10), child.wait_with_output()).await??;
    assert!(
        output.status.success(),
        "SIGTERM deve concluir shutdown gracioso"
    );
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stdout.contains("shutdown gracioso solicitado"));
    for log in [&stdout, &stderr] {
        assert!(!log.contains(token));
        assert!(!log.contains("integration-only"));
        assert!(!log.contains(url));
    }
    for line in stdout.lines() {
        let _: serde_json::Value = serde_json::from_str(line)?;
    }
    Ok(())
}

#[tokio::test]
async fn fingerprint_cli_uses_only_environment_and_never_prints_token()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let token = "fixture-credential-only-for-cli";
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_stories-tech"))
            .env_clear()
            .env("IG_ACCESS_TOKEN", token)
            .env("STORIES_CONFIG", root.join("config/exemplo.toml"))
            .env("STORIES_SOURCES", root.join("config/sources.toml"))
            .arg("--token-fingerprint")
            .output(),
    )
    .await??;
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)?.trim(),
        blake3::hash(token.as_bytes()).to_hex().as_str()
    );
    assert!(!String::from_utf8(output.stderr)?.contains(token));
    Ok(())
}
