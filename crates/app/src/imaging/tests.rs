use super::*;
use stories_core::CapturedArticle;
use stories_editorial::{CheckedSegment, Citation, Claim, Editorial, FactReport};
use stories_imaging::render::example_draft;
use stories_normalize::Normalizer;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

async fn seed(store: &Store, source: i32, index: i32, wide: bool) -> Result<i64> {
    let mut draft = example_draft();
    if wide {
        draft.title = format!("{} lança nova versão", "W".repeat(24));
    }
    let excerpt = format!("Rust. {} {}", draft.title, draft.summary.join(" "));
    let article = Normalizer::new()?.normalize(CapturedArticle {
        url: format!("https://blog.rust-lang.org/image-{index}"),
        title: draft.title.clone(),
        excerpt: excerpt.clone(),
        published_at: Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
        image_url: None,
    })?;
    store
        .ingest_batch(source, &[article], None, None, false, chrono::Utc::now())
        .await?;
    let article_id: i64 = sqlx::query_scalar("SELECT id FROM articles WHERE url_canonical=$1")
        .bind(format!("https://blog.rust-lang.org/image-{index}"))
        .fetch_one(store.pool())
        .await?;
    let event = store.create_event(None).await?;
    sqlx::query("UPDATE events SET primary_article_id=$2 WHERE id=$1")
        .bind(event)
        .bind(article_id)
        .execute(store.pool())
        .await?;
    sqlx::query("INSERT INTO event_articles(event_id,article_id) VALUES($1,$2)")
        .bind(event)
        .bind(article_id)
        .execute(store.pool())
        .await?;
    sqlx::query("UPDATE events SET state='CLUSTERED' WHERE id=$1")
        .bind(event)
        .execute(store.pool())
        .await?;
    sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('verify',$1,$2)")
        .bind(format!("image:seed:{event}"))
        .bind(json!({"event_id":event}))
        .execute(store.pool())
        .await?;
    let job = claim(store, event, "verify").await?;
    let context = store.editorial_context(event).await?;
    let mut evidence = context.evidence.clone();
    for source in &mut evidence {
        source.date_confirmed = true;
    }
    let assessment = stories_verify::assess(&evidence, chrono::Utc::now(), 6);
    store
        .editorial_commit(&job, &context, EditorialStep::Verified(assessment.clone()))
        .await?;
    let validator = Editorial::new()?;
    let draft = validator.parse_draft(serde_json::to_value(draft)?, &assessment, "new")?;
    validator.deterministic(&draft, &assessment)?;
    let report = FactReport {
        language_ok: true,
        neutral: true,
        segments: draft
            .segments()
            .into_iter()
            .enumerate()
            .map(|(index, text)| CheckedSegment {
                index,
                text: text.clone(),
                supported: true,
                claims: vec![Claim {
                    text,
                    supported: true,
                    evidence: vec![Citation {
                        article_id,
                        quote: excerpt.clone(),
                    }],
                }],
            })
            .collect(),
    };
    let report = validator.parse_report(serde_json::to_value(report)?, &draft, &assessment)?;
    store
        .editorial_commit(
            &job,
            &store.editorial_context(event).await?,
            EditorialStep::Drafted(draft),
        )
        .await?;
    store
        .editorial_commit(
            &job,
            &store.editorial_context(event).await?,
            EditorialStep::Checked(report),
        )
        .await?;
    Ok(event)
}

async fn claim(store: &Store, event: i64, kind: &str) -> Result<Job> {
    Ok(sqlx::query_as("UPDATE jobs SET lease_token=$3,lease_until=now()+interval '120 seconds',attempts=attempts+1 WHERE id=(SELECT id FROM jobs WHERE kind=$2 AND payload->>'event_id'=$1 AND done_at IS NULL AND dead_at IS NULL AND (lease_until IS NULL OR lease_until<=now()) ORDER BY id LIMIT 1) RETURNING id,kind,payload,attempts,lease_token,lease_until")
        .bind(event.to_string()).bind(kind).bind(uuid::Uuid::new_v4()).fetch_one(store.pool()).await?)
}

async fn count(store: &Store, event: i64, kind: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'event_id'=$1 AND kind=$2")
            .bind(event.to_string())
            .bind(kind)
            .fetch_one(store.pool())
            .await?,
    )
}

#[tokio::test]
async fn checked_to_jpeg_is_durable_atomic_fenced_and_dry_run() -> Result<()> {
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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut settings = Settings::load(
        &root.join("config/exemplo.toml"),
        &root.join("config/sources.toml"),
    )?;
    let temporary =
        std::env::temp_dir().join(format!("breaking-bytes-images-{}", uuid::Uuid::new_v4()));
    settings.imaging.artifacts_dir = temporary
        .to_str()
        .context("diretório temporário inválido")?
        .into();
    let processor = Processor {
        catalog: Catalog::default(),
        renderer: Arc::new(Mutex::new(Renderer::new()?)),
        http: HttpClient::shared("BreakingBytes/0.1", 2097152, 2)?,
    };
    let first = seed(&store, source, 1, false).await?;
    let job = claim(&store, first, "render").await?;
    processor.process(&settings, &store, &job).await?;
    assert_eq!(
        store.editorial_context(first).await?.state,
        EventState::Rendered
    );
    let (path, hash, dry_run): (String, Vec<u8>, bool) = sqlx::query_as(
        "SELECT artifact_path,content_hash,dry_run FROM rendered_stories WHERE event_id=$1",
    )
    .bind(first)
    .fetch_one(store.pool())
    .await?;
    let jpeg = tokio::fs::read(temporary.join(&path)).await?;
    assert!(jpeg.starts_with(&[0xff, 0xd8]));
    assert_eq!(blake3::hash(&jpeg).as_bytes().as_slice(), hash);
    assert!(dry_run);
    assert_eq!(count(&store, first, "schedule").await?, 1);
    let publications: i64 = sqlx::query_scalar("SELECT count(*) FROM publications")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(publications, 0);
    sqlx::query("INSERT INTO jobs(kind,dedupe_key,payload) VALUES('render',$1,$2)")
        .bind(format!("image:duplicate:{first}"))
        .bind(json!({"event_id":first,"revision":store.editorial_context(first).await?.revision}))
        .execute(store.pool())
        .await?;
    let duplicate = claim(&store, first, "render").await?;
    processor.process(&settings, &store, &duplicate).await?;
    assert_eq!(count(&store, first, "schedule").await?, 1);

    let wide = seed(&store, source, 2, true).await?;
    let job = claim(&store, wide, "render").await?;
    processor.process(&settings, &store, &job).await?;
    assert_eq!(
        store.editorial_context(wide).await?.state,
        EventState::Superseded
    );
    let child:i64=sqlx::query_scalar("SELECT (detail->>'replacement')::bigint FROM audit_log WHERE event_id=$1 AND step='render_rewrite'").bind(wide).fetch_one(store.pool()).await?;
    let context = store.editorial_context(child).await?;
    assert_eq!(context.state, EventState::Clustered);
    assert_eq!(context.rewrites, 1);
    assert!(context.draft.is_none());
    let payload: Value = sqlx::query_scalar("SELECT payload FROM jobs WHERE dedupe_key=$1")
        .bind(format!("verify:layout:{child}"))
        .fetch_one(store.pool())
        .await?;
    assert_eq!(payload["shorten"], true);

    let exhausted = seed(&store, source, 3, true).await?;
    sqlx::query("UPDATE events SET editorial_attempts=2 WHERE id=$1")
        .bind(exhausted)
        .execute(store.pool())
        .await?;
    let job = claim(&store, exhausted, "render").await?;
    processor.process(&settings, &store, &job).await?;
    assert_eq!(
        store.editorial_context(exhausted).await?.state,
        EventState::Rejected
    );

    let stale = seed(&store, source, 4, false).await?;
    sqlx::query("UPDATE articles SET body_excerpt=body_excerpt||' Novo fato.' WHERE id IN(SELECT article_id FROM event_articles WHERE event_id=$1)").bind(stale).execute(store.pool()).await?;
    let job = claim(&store, stale, "render").await?;
    processor.process(&settings, &store, &job).await?;
    assert_eq!(
        store.editorial_context(stale).await?.state,
        EventState::Superseded
    );
    assert_eq!(count(&store, stale, "schedule").await?, 0);

    // Se o lease vencer depois do JPEG, o banco não avança; a retomada usa o mesmo hash.
    let crashed = seed(&store, source, 5, false).await?;
    let old = claim(&store, crashed, "render").await?;
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(old.id)
        .execute(store.pool())
        .await?;
    let error = processor
        .process(&settings, &store, &old)
        .await
        .err()
        .context("lease vencido deveria bloquear")?;
    assert!(matches!(
        error.downcast_ref::<StoreError>(),
        Some(StoreError::LostLease)
    ));
    assert_eq!(
        store.editorial_context(crashed).await?.state,
        EventState::Checked
    );
    assert_eq!(count(&store, crashed, "schedule").await?, 0);
    let recovered = claim(&store, crashed, "render").await?;
    assert_ne!(old.lease_token, recovered.lease_token);
    processor.process(&settings, &store, &recovered).await?;
    assert_eq!(
        store.editorial_context(crashed).await?.state,
        EventState::Rendered
    );
    assert_eq!(count(&store, crashed, "schedule").await?, 1);

    // Uma falha na outbox reverte também o estado e a linha do artefato.
    let rollback = seed(&store, source, 6, false).await?;
    let job = claim(&store, rollback, "render").await?;
    sqlx::query(
        "ALTER TABLE jobs ADD CONSTRAINT deny_schedule_fixture CHECK(kind<>'schedule') NOT VALID",
    )
    .execute(store.pool())
    .await?;
    assert!(processor.process(&settings, &store, &job).await.is_err());
    assert_eq!(
        store.editorial_context(rollback).await?.state,
        EventState::Checked
    );
    let rendered: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rendered_stories WHERE event_id=$1")
            .bind(rollback)
            .fetch_one(store.pool())
            .await?;
    assert_eq!(rendered, 0);
    sqlx::query("ALTER TABLE jobs DROP CONSTRAINT deny_schedule_fixture")
        .execute(store.pool())
        .await?;
    processor.process(&settings, &store, &job).await?;
    assert_eq!(
        store.editorial_context(rollback).await?.state,
        EventState::Rendered
    );

    // Evidência nova depois do render cria verificação e invalida o candidato antigo.
    sqlx::query("UPDATE articles SET body_excerpt=body_excerpt||' Atualização.' WHERE id IN(SELECT article_id FROM event_articles WHERE event_id=$1)").bind(first).execute(store.pool()).await?;
    let context = store.editorial_context(first).await?;
    assert_ne!(context.verified_revision, Some(context.revision));
    let verification = claim(&store, first, "verify").await?;
    store
        .editorial_commit(&verification, &context, EditorialStep::Skip)
        .await?;
    assert_eq!(
        store.editorial_context(first).await?.state,
        EventState::Superseded
    );
    store.set_paused(true).await?;
    assert!(
        store
            .claim_kind(Duration::from_secs(120), 5, "render")
            .await?
            .is_none()
    );
    store.set_paused(false).await?;
    store.close().await;
    tokio::fs::remove_dir_all(temporary).await?;
    Ok(())
}
