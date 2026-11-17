use super::*;
use serde_json::json;
use stories_core::EventState;
use stories_store::Enqueue;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

async fn seed(store: &Store, dry_run: bool, index: i32, root: &Path) -> Result<(i64, Job)> {
    let event = store.create_event(None).await?;
    let source = store
        .register_source(
            &format!("publication-{index}"),
            "A",
            "rss",
            &format!("https://example.com/{index}"),
            5,
            60,
        )
        .await?;
    let article:i64=sqlx::query_scalar("INSERT INTO articles(source_id,url_canonical,title,title_hash,published_at) VALUES($1,$2,'Rust',decode(repeat('00',32),'hex'),now()-interval '1 minute') RETURNING id").bind(source).bind(format!("https://example.com/{index}")).fetch_one(store.pool()).await?;
    sqlx::query("INSERT INTO event_articles(event_id,article_id) VALUES($1,$2)")
        .bind(event)
        .bind(article)
        .execute(store.pool())
        .await?;
    for (from, to) in [
        (EventState::Detected, EventState::Clustered),
        (EventState::Clustered, EventState::Verified),
        (EventState::Verified, EventState::Drafted),
        (EventState::Drafted, EventState::Checked),
        (EventState::Checked, EventState::Rendered),
    ] {
        store.transition(event, from, to, None, None).await?;
    }
    sqlx::query("UPDATE events SET verified_revision=content_revision WHERE id=$1")
        .bind(event)
        .execute(store.pool())
        .await?;
    let bytes = include_bytes!("../../../../docs/reports/examples/story.jpg");
    let relative = format!("{event}.jpg");
    tokio::fs::write(root.join(&relative), bytes).await?;
    sqlx::query("INSERT INTO rendered_stories(event_id,revision,template,artifact_path,content_hash,byte_length,width,height,layout,image_provenance,dry_run) SELECT id,content_revision,'story-v1',$2,$3,$4,1080,1920,'{}','{}',$5 FROM events WHERE id=$1")
        .bind(event).bind(relative).bind(blake3::hash(bytes).as_bytes().as_slice()).bind(bytes.len() as i64).bind(dry_run).execute(store.pool()).await?;
    let revision: i64 = sqlx::query_scalar("SELECT content_revision FROM events WHERE id=$1")
        .bind(event)
        .fetch_one(store.pool())
        .await?;
    store
        .enqueue(
            Enqueue {
                kind: "schedule",
                key: &format!("schedule:test:{event}"),
                payload: &json!({"event_id":event,"revision":revision}),
            },
            chrono::Utc::now(),
        )
        .await?;
    let job = store
        .claim_kind(Duration::from_secs(30), 5, "schedule")
        .await?
        .context("job schedule ausente")?;
    Ok((event, job))
}
fn options(account: &str, dry_run: bool) -> PublicationPolicy<'_> {
    PublicationPolicy {
        account,
        dry_run,
        policy: Policy {
            daily_cap: 40,
            reserved: 5,
            breaking_score: 0.8,
            min_gap_seconds: 90,
        },
        remote_usage: 0,
        remote_limit: 100,
        max_age_hours: 6,
    }
}
async fn reserve(
    store: &Store,
    event: i64,
    job: &Job,
    account: &str,
    dry_run: bool,
) -> Result<i64> {
    store
        .schedule_publication(
            job,
            event,
            1,
            0.9,
            json!({"test":true}),
            options(account, dry_run),
        )
        .await?;
    Ok(
        sqlx::query_scalar("SELECT id FROM publications WHERE event_id=$1")
            .bind(event)
            .fetch_one(store.pool())
            .await?,
    )
}
async fn publish_job(store: &Store) -> Result<Job> {
    store
        .claim_kind(Duration::from_secs(30), 5, "publish")
        .await?
        .context("job publish ausente")
}
async fn expire(store: &Store, job: &Job) -> Result<Job> {
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second',run_at=now() WHERE id=$1")
        .bind(job.id)
        .execute(store.pool())
        .await?;
    store
        .claim_kind(Duration::from_secs(30), 5, "reconcile")
        .await?
        .context("job reconcile ausente")
}

#[tokio::test]
async fn dry_run_and_publication_crash_recovery_use_postgres_and_http() -> Result<()> {
    let _ = stories_collectors::http::HttpClient::shared("BreakingBytes/0.1", 1024, 1)?;
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
    let root = std::env::temp_dir().join(format!("stories-publication-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await?;
    let mut settings = Settings::load(
        Path::new("../../config/exemplo.toml"),
        Path::new("../../config/sources.toml"),
    )?;
    settings.imaging.artifacts_dir = root.to_string_lossy().into_owned();
    let dry = Publisher {
        settings: settings.clone(),
        account: "dry_run-local".into(),
        remote: None,
        healthy: Arc::new(AtomicBool::new(true)),
    };
    let (event, schedule) = seed(&store, true, 1, &root).await?;
    let id = reserve(&store, event, &schedule, &dry.account, true).await?;
    assert!(
        store
            .schedule_publication(
                &schedule,
                event,
                1,
                0.9,
                json!({}),
                options(&dry.account, true)
            )
            .await
            .is_err(),
        "lease antigo não pode reservar novamente"
    );
    let job = publish_job(&store).await?;
    dry.process(&store, &job).await?;
    let p = store.publication(id).await?;
    assert_eq!(p.status, "dry_run");
    assert!(p.container_id.is_none() && p.media_id.is_none() && p.image_key.is_none());
    let state: String = sqlx::query_scalar("SELECT state FROM events WHERE id=$1")
        .bind(event)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(state, "QUEUED");
    let server = MockServer::start().await;
    let graph = Graph::test_server(
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        &format!("{}/v27.0/", server.uri()),
    )?;
    let account = graph.account_key();
    let r2 = R2::new(
        reqwest::Client::new(),
        &"a".repeat(32),
        "stories-img",
        "test",
        "test",
        Duration::from_secs(3600),
    )?;
    settings.instagram.dry_run = false;
    let live = Publisher {
        settings,
        account: account.clone(),
        remote: Some(Remote {
            graph,
            r2,
            token: crate::token::TokenMonitor::test_facebook(
                stories_publisher::token::TokenClient::test_server(
                    reqwest::Client::builder().no_proxy().build()?,
                    Login::Facebook,
                    &format!("{}/", server.uri()),
                )?,
            )?,
        }),
        healthy: Arc::new(AtomicBool::new(true)),
    };
    Mock::given(path("/v27.0/123/content_publishing_limit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[{"quota_usage":0,"config":{"quota_total":100,"quota_duration":86400}}]}),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/v27.0/123/stories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"999"}]})))
        .mount(&server)
        .await;
    Mock::given(path("/debug_token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"app_id":"99","type":"USER","is_valid":true,"expires_at":(chrono::Utc::now()+chrono::Duration::days(30)).timestamp(),"scopes":["instagram_basic","instagram_content_publish","pages_read_engagement"]}}))).mount(&server).await;
    live.remote
        .as_ref()
        .context("remote ausente")?
        .token
        .check(&store, &account)
        .await?;
    // Queda depois do efeito remoto e antes de persistir a resposta: outro dono reconcilia.
    let (event, schedule) = seed(&store, false, 2, &root).await?;
    let id = reserve(&store, event, &schedule, &account, false).await?;
    let job = publish_job(&store).await?;
    store
        .publication_step(
            &job,
            id,
            PublicationStep::UploadPlanned("stories/test.jpg"),
            options(&account, false),
        )
        .await?;
    store
        .publication_step(
            &job,
            id,
            PublicationStep::Container("456"),
            options(&account, false),
        )
        .await?;
    store
        .publication_step(&job, id, PublicationStep::Ready, options(&account, false))
        .await?;
    store
        .publication_step(&job, id, PublicationStep::Begin, options(&account, false))
        .await?;
    Mock::given(method("POST"))
        .and(path("/v27.0/123/media_publish"))
        .and(body_json(json!({"creation_id":"456"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"999"})))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        live.remote
            .as_ref()
            .context("remote ausente")?
            .graph
            .publish("456")
            .await?,
        "999"
    );
    // Simula perda de memória do processo: a resposta acima é descartada.
    store.set_paused(true).await?;
    let recovered = expire(&store, &job).await?;
    assert!(matches!(
        store
            .publication_step(
                &job,
                id,
                PublicationStep::Published("999"),
                options(&account, false)
            )
            .await,
        Err(StoreError::LostLease)
    ));
    Mock::given(path("/v27.0/456"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status_code":"PUBLISHED"})))
        .mount(&server)
        .await;
    store.set_paused(true).await?;
    live.process(&store, &recovered).await?;
    let p = store.publication(id).await?;
    assert_eq!(p.status, "published_unresolved");
    assert!(p.media_id.is_none());
    let state: String = sqlx::query_scalar("SELECT state FROM events WHERE id=$1")
        .bind(event)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(state, "PUBLISHED");
    server.verify().await;
    store.set_paused(false).await?;
    // Queda depois da barreira, antes da chamada: FINISHED não autoriza repetir POST.
    let (event, schedule) = seed(&store, false, 3, &root).await?;
    let id = reserve(&store, event, &schedule, &account, false).await?;
    let job = publish_job(&store).await?;
    store
        .publication_step(
            &job,
            id,
            PublicationStep::UploadPlanned("stories/ambiguous.jpg"),
            options(&account, false),
        )
        .await?;
    store
        .publication_step(
            &job,
            id,
            PublicationStep::Container("457"),
            options(&account, false),
        )
        .await?;
    store
        .publication_step(&job, id, PublicationStep::Ready, options(&account, false))
        .await?;
    store
        .publication_step(&job, id, PublicationStep::Begin, options(&account, false))
        .await?;
    let recovered = expire(&store, &job).await?;
    Mock::given(path("/v27.0/457"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status_code":"FINISHED"})))
        .mount(&server)
        .await;
    live.process(&store, &recovered).await?;
    assert_eq!(store.publication(id).await?.status, "uncertain");
    let candidates = store.cleanup_candidates(&account, 0).await?;
    assert!(
        !candidates.iter().any(|p| p.id == id),
        "incerteza preserva imagem"
    );
    server.verify().await;
    // Reinício após persistir container: reutiliza o ID e confirma o único POST.
    let (event, schedule) = seed(&store, false, 5, &root).await?;
    let id = reserve(&store, event, &schedule, &account, false).await?;
    let old = publish_job(&store).await?;
    store
        .publication_step(
            &old,
            id,
            PublicationStep::UploadPlanned("stories/resume.jpg"),
            options(&account, false),
        )
        .await?;
    store
        .publication_step(
            &old,
            id,
            PublicationStep::Container("458"),
            options(&account, false),
        )
        .await?;
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second',run_at=now() WHERE id=$1")
        .bind(old.id)
        .execute(store.pool())
        .await?;
    let resumed = publish_job(&store).await?;
    Mock::given(path("/v27.0/458"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status_code":"FINISHED"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v27.0/123/media_publish"))
        .and(body_json(json!({"creation_id":"458"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"1001"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v27.0/123/media"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    live.process(&store, &resumed).await?;
    let p = store.publication(id).await?;
    assert_eq!(p.status, "published");
    assert_eq!(p.media_id.as_deref(), Some("1001"));
    server.verify().await;
    // A cota remota bloqueia inclusive uma notícia de score máximo.
    let (event, schedule) = seed(&store, true, 6, &root).await?;
    let capped = PublicationPolicy {
        remote_usage: 100,
        remote_limit: 100,
        ..options("quota-test", true)
    };
    store
        .schedule_publication(&schedule, event, 1, 1.0, json!({}), capped)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM publications WHERE event_id=$1")
        .bind(event)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(count, 0);
    // Mudança de revisão depois da reserva impede qualquer efeito remoto.
    let (event, schedule) = seed(&store, true, 4, &root).await?;
    let id = reserve(&store, event, &schedule, "dry_run-other", true).await?;
    let job = publish_job(&store).await?;
    sqlx::query("UPDATE events SET content_revision=content_revision+1 WHERE id=$1")
        .bind(event)
        .execute(store.pool())
        .await?;
    assert!(
        !store
            .publication_step(
                &job,
                id,
                PublicationStep::Simulated,
                options("dry_run-other", true)
            )
            .await?
    );
    assert_eq!(store.publication(id).await?.status, "failed");
    let context = store.editorial_context(event).await?;
    assert_eq!(
        context.state,
        EventState::Queued,
        "a reavaliação precisa encontrar o evento ativo"
    );
    let verify = store
        .claim_kind(Duration::from_secs(30), 5, "verify")
        .await?
        .context("reavaliação perdida")?;
    assert_eq!(
        verify.payload.get("event_id").and_then(Value::as_i64),
        Some(event)
    );
    assert_eq!(
        store
            .editorial_commit(&verify, &context, stories_store::EditorialStep::Skip)
            .await?,
        stories_store::EditorialOutcome::Restarted
    );
    let replacements:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE event_id=$1 AND step='evidence_superseded' AND detail ? 'replacement'").bind(event).fetch_one(store.pool()).await?;
    assert_eq!(
        replacements, 1,
        "a nova evidência precisa de candidato substituto"
    );
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log a JOIN events e ON e.id=(a.detail->>'replacement')::bigint JOIN jobs j ON (j.payload->>'event_id')::bigint=e.id WHERE a.event_id=$1 AND a.step='evidence_superseded' AND e.state='CLUSTERED' AND j.kind='verify' AND j.done_at IS NULL AND j.dead_at IS NULL)").bind(event).fetch_one(store.pool()).await?;
    assert!(
        pending,
        "o substituto precisa continuar na outbox de verificação"
    );
    assert!(
        read_artifact(&root, "../escape.jpg", &[0; 32], 10)
            .await
            .is_err()
    );
    assert!(read_artifact(&root, "1.jpg", &[0; 32], 1).await.is_err());
    store.close().await;
    tokio::fs::remove_dir_all(root).await?;
    Ok(())
}
