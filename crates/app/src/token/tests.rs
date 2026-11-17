use super::*;
use serde_json::json;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn instagram(now: chrono::DateTime<Utc>) -> TokenObservation {
    TokenObservation {
        kind: TokenKind::Instagram,
        source: TokenSource::OAuthEnvironment,
        valid: true,
        app_id: None,
        profile_id: None,
        issued_at: Some(now - Duration::days(30)),
        expires_at: Some(now + Duration::days(30)),
        data_access_expires_at: None,
        checked_at: now,
        refreshed_at: None,
        scopes: vec![
            "instagram_business_basic".into(),
            "instagram_business_content_publish".into(),
        ],
        rotation_required: false,
        failure: None,
    }
}
#[tokio::test]
async fn token_monitor_refresh_fencing_restart_rotation_and_alerts_use_postgres() -> Result<()> {
    let _ = HttpClient::shared("BreakingBytes/0.1", 1024, 1)?;
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
    let store = Store::connect(&url, 10, std::time::Duration::from_secs(10)).await?;
    store.migrate().await?;
    let server = MockServer::start().await;
    let client = TokenClient::test_server(
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        Login::Instagram,
        &format!("{}/", server.uri()),
    )?;
    let now = Utc::now();
    let credential = Credential::new("fixture-current-secret")?;
    let fingerprint = credential.fingerprint();
    let receipt = instagram(now);
    let monitor = TokenMonitor {
        client: client.clone(),
        credential: credential.clone(),
        source: Source::Instagram {
            receipt: receipt.clone(),
        },
        settings: TokenSettings::default(),
        state: Arc::new(TokenState::default()),
        first_check: AtomicBool::new(true),
    };
    Mock::given(method("GET")).and(path("/refresh_access_token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"fixture-current-secret","token_type":"bearer","expires_in":5183944}))).expect(1).mount(&server).await;
    monitor.check(&store, "account-a").await?;
    let saved = store
        .token_record("account-a", fingerprint)
        .await?
        .context("metadados ausentes")?;
    assert!(saved.observation.refreshed_at.is_some());
    assert!(saved.observation.expires_at > receipt.expires_at);
    assert_eq!(saved.observation.source, TokenSource::InstagramRefresh);
    // Reinício com o mesmo ambiente antigo conserva a validade recebida do servidor.
    let restarted = TokenMonitor {
        client: client.clone(),
        credential: credential.clone(),
        source: Source::Instagram {
            receipt: receipt.clone(),
        },
        settings: TokenSettings::default(),
        state: Arc::new(TokenState::default()),
        first_check: AtomicBool::new(true),
    };
    restarted.check(&store, "account-a").await?;
    restarted.ensure(&store, "account-a").await?;
    assert_eq!(
        store
            .token_record("account-a", fingerprint)
            .await?
            .context("registro ausente")?
            .observation
            .expires_at,
        saved.observation.expires_at
    );
    server.verify().await;
    // A validade é reavaliada no health check, mesmo antes do próximo preflight.
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let config = Settings::load(
        std::path::Path::new("../../config/exemplo.toml"),
        std::path::Path::new("../../config/sources.toml"),
    )?;
    let admin = "a".repeat(32);
    let secrets = config.resolve_with(|name| match name {
        "DATABASE_URL" => Some(url.clone()),
        "ADMIN_TOKEN" => Some(admin.clone()),
        _ => None,
    })?;
    let app = crate::router(crate::AppState {
        store: store.clone(),
        admin_token: secrets.admin_token,
        dry_run: false,
        collection_enabled: false,
        editorial_enabled: false,
        imaging_enabled: false,
        publishing_enabled: true,
        corrections_enabled: false,
        publisher_admin: std::sync::Arc::new(crate::publication::AdminPublisher::default()),
        publishing_ready: Arc::new(AtomicBool::new(true)),
        token_state: monitor.state.clone(),
    });
    let request = || {
        axum::http::Request::builder()
            .uri("/health/ready")
            .body(axum::body::Body::empty())
    };
    assert_eq!(
        app.clone().oneshot(request()?).await?.status(),
        axum::http::StatusCode::OK
    );
    let mut expired = saved.observation.clone();
    expired.expires_at = Some(Utc::now() - Duration::seconds(1));
    monitor.state.update(expired).await;
    assert_eq!(
        app.clone().oneshot(request()?).await?.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/status")
                .header("authorization", format!("Bearer {admin}"))
                .body(axum::body::Body::empty())?,
        )
        .await?;
    let bytes = response.into_body().collect().await?.to_bytes();
    let status: serde_json::Value = serde_json::from_slice(&bytes)?;
    assert_eq!(status["token_health"]["status"], "expired");
    assert!(!status.to_string().contains("fixture-current-secret"));
    monitor.state.update(saved.observation.clone()).await;
    // Réplicas só podem reservar uma renovação; queda após a barreira bloqueia retomada cega.
    let row = store
        .save_token_observation("account-b", fingerprint, None, &receipt)
        .await?;
    let (a, b) = tokio::join!(
        store.begin_token_refresh("account-b", fingerprint, row.version, 30),
        store.begin_token_refresh("account-b", fingerprint, row.version, 30)
    );
    let (a, b) = (a?, b?);
    assert_eq!(usize::from(a.is_some()) + usize::from(b.is_some()), 1);
    let lease = a.or(b).context("lease ausente")?;
    assert!(
        !store
            .token_record("account-b", fingerprint)
            .await?
            .context("registro ausente")?
            .observation
            .publication_allowed(now, 620)
    );
    sqlx::query("UPDATE token_health SET refresh_lease_until=now()-interval '1 second' WHERE account_key='account-b'").execute(store.pool()).await?;
    assert!(matches!(
        store
            .finish_token_refresh("account-b", fingerprint, &lease, &receipt)
            .await,
        Err(StoreError::LostLease)
    ));
    assert!(restarted.check(&store, "account-b").await.is_err());
    // Resposta com nova credencial não é instalada ou persistida.
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/refresh_access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"access_token":"fixture-new-secret","token_type":"bearer","expires_in":5183944}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    assert!(monitor.check(&store, "account-c").await.is_err());
    let blocked = store
        .token_record("account-c", fingerprint)
        .await?
        .context("registro ausente")?;
    assert!(blocked.observation.rotation_required);
    assert_eq!(
        blocked.observation.failure,
        Some(TokenFailure::RotationRequired)
    );
    assert!(monitor.ensure(&store, "account-c").await.is_err());
    let rows: String = sqlx::query_scalar("SELECT jsonb_agg(observation)::text FROM token_health")
        .fetch_one(store.pool())
        .await?;
    let audit: String = sqlx::query_scalar("SELECT jsonb_agg(detail)::text FROM audit_log")
        .fetch_one(store.pool())
        .await?;
    for secret in ["fixture-current-secret", "fixture-new-secret"] {
        assert!(!rows.contains(secret));
        assert!(!audit.contains(secret));
    }
    // Trocar o segredo no ambiente muda o hash e não herda a intenção bloqueada.
    let new_credential = Credential::new("fixture-operator-secret")?;
    assert!(
        store
            .token_record("account-c", new_credential.fingerprint())
            .await?
            .is_none()
    );
    let mut newer = instagram(Utc::now());
    newer.issued_at = Some(Utc::now() - Duration::days(1));
    newer.expires_at = Some(Utc::now() + Duration::days(59));
    store
        .save_token_observation("account-c", new_credential.fingerprint(), None, &newer)
        .await?;
    assert!(
        store
            .token_alert_due("account-c", new_credential.fingerprint(), "expiring")
            .await?
    );
    assert!(
        !store
            .token_alert_due("account-c", new_credential.fingerprint(), "expiring")
            .await?
    );
    assert!(
        store
            .token_alert_due("account-c", new_credential.fingerprint(), "invalid")
            .await?
    );
    let mut expired = newer.clone();
    expired.expires_at = Some(Utc::now() - Duration::seconds(1));
    monitor.state.update(expired).await;
    assert!(!monitor.state.allowed().await);
    assert_eq!(
        monitor.state.view().await.context("estado ausente")?.status,
        TokenStatus::Expired
    );
    // Revogação da API não é apagada por um recibo estático no próximo ciclo.
    monitor
        .invalidate(&store, "account-a", TokenFailure::Invalid)
        .await?;
    assert!(restarted.check(&store, "account-a").await.is_err());
    server.verify().await;
    // Cancelamento real do futuro durante HTTP: readiness cai antes da resposta e a retomada não repete.
    server.reset().await;
    Mock::given(method("GET")).and(path("/refresh_access_token")).respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(11)).set_body_json(json!({"access_token":"fixture-current-secret","token_type":"bearer","expires_in":5183944}))).expect(1).mount(&server).await;
    {
        let work = monitor.check(&store, "account-d");
        tokio::pin!(work);
        let observed = async {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    let received = server
                        .received_requests()
                        .await
                        .context("requisições indisponíveis")?;
                    if !received.is_empty() && !monitor.state.allowed().await {
                        return Ok::<_, anyhow::Error>(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .context("barreira não observada durante HTTP")??;
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! {
            result=&mut work=>{result?;anyhow::bail!("renovação não deveria concluir antes da interrupção");}
            result=observed=>result?,
        }
    }
    assert_eq!(
        monitor.state.view().await.context("estado ausente")?.status,
        TokenStatus::RotationRequired
    );
    assert!(restarted.check(&store, "account-d").await.is_err());
    let persisted = store
        .token_record("account-d", fingerprint)
        .await?
        .context("barreira perdida")?;
    assert!(persisted.observation.rotation_required);
    assert_eq!(
        persisted.observation.failure,
        Some(TokenFailure::RefreshUncertain)
    );
    server.verify().await;
    store.close().await;
    Ok(())
}

#[test]
fn verbose_dependency_logs_cannot_expose_query_or_credentials() -> Result<()> {
    use std::io::Write;
    use std::sync::Mutex;
    use tracing_subscriber::prelude::*;
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("lock"))?
                .write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(bytes.clone());
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(|m| {
                log_target_allowed(m.target())
            })),
    );
    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!(target:"reqwest::connect",url="https://graph.facebook.com/debug_token?input_token=fixture-secret","request");
        tracing::warn!(target:"aws_sdk_s3",token="fixture-secret","sdk");
        tracing::warn!(target:"figment",value="fixture-secret","config");
        tracing::info!(target:"stories_app",code="expired","credencial exige atenção");
    });
    let output = String::from_utf8(bytes.lock().map_err(|_| anyhow::anyhow!("lock"))?.clone())?;
    assert!(!output.contains("fixture-secret"));
    assert!(!output.contains("input_token"));
    assert!(output.contains("expired"));
    Ok(())
}
