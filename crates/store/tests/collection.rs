use chrono::Utc;
use std::time::Duration;
use stories_core::{CapturedArticle, EventState};
use stories_normalize::Normalizer;
use stories_store::Store;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

#[tokio::test]
async fn atomic_ingest_concurrent_clustering_updates_and_burst()
-> Result<(), Box<dyn std::error::Error>> {
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
    let first = store
        .register_source("Primary", "A", "rss", "https://example.com/feed", 5, 60)
        .await?;
    let second = store
        .register_source(
            "Secondary",
            "B",
            "rss",
            "https://other.example.com/feed",
            3,
            60,
        )
        .await?;
    let n = Normalizer::new()?;
    let make = |path: &str, title: &str| {
        n.normalize(CapturedArticle {
            url: format!("https://example.com/{path}"),
            title: title.into(),
            excerpt: String::new(),
            image_url: None,
            published_at: Some(Utc::now()),
        })
    };
    let a = make("a?utm_source=tracker", "Rust anuncia versão 1.100")?;
    let b = make("b", "Rust anuncia versão 1.100!")?;
    assert_eq!(
        store
            .ingest_batch(
                first,
                std::slice::from_ref(&a),
                Some("v1"),
                None,
                false,
                Utc::now()
            )
            .await?
            .inserted,
        1
    );
    let stats = store
        .ingest_batch(
            second,
            &[a.clone(), b],
            Some("secondary-v1"),
            None,
            false,
            Utc::now(),
        )
        .await?;
    assert_eq!((stats.inserted, stats.duplicates), (1, 1));
    let sightings: i64 = sqlx::query_scalar("SELECT count(*) FROM article_sightings")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(sightings, 3);

    sqlx::query("CREATE FUNCTION fail_article_fixture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.title='storage-failure' THEN RAISE EXCEPTION 'fixture' USING ERRCODE='23514'; END IF; RETURN NEW; END; $$")
        .execute(store.pool()).await?;
    sqlx::query("CREATE TRIGGER article_failure BEFORE INSERT ON articles FOR EACH ROW EXECUTE FUNCTION fail_article_fixture()")
        .execute(store.pool()).await?;
    let transient = make("transient", "Rust anuncia versão 1.100")?;
    let failed = make("fail", "storage-failure")?;
    assert!(
        store
            .ingest_batch(
                first,
                &[transient, failed],
                Some("v2"),
                None,
                false,
                Utc::now()
            )
            .await
            .is_err()
    );
    assert_eq!(store.source_state(first).await?.etag.as_deref(), Some("v1"));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM articles")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(count, 2);
    sqlx::query("DROP TRIGGER article_failure ON articles")
        .execute(store.pool())
        .await?;
    sqlx::query("DROP FUNCTION fail_article_fixture()")
        .execute(store.pool())
        .await?;

    let lease = Duration::from_secs(120);
    let (left, right) = tokio::join!(
        store.claim_kind(lease, 5, "cluster"),
        store.claim_kind(lease, 5, "cluster")
    );
    let left = left?.ok_or("primeiro job ausente")?;
    let right = right?.ok_or("segundo job ausente")?;
    let (first_event, second_event) = tokio::join!(
        store.cluster_job(&left, 0.86, 6),
        store.cluster_job(&right, 0.86, 6)
    );
    let event = first_event?;
    assert_eq!(event, second_event?);
    assert!(store.cluster_job(&left, 0.86, 6).await.is_err());
    assert!(store.claim_kind(lease, 5, "cluster").await?.is_none());
    let verify_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='verify'")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(verify_jobs, 2);
    let primary:i32=sqlx::query_scalar("SELECT a.source_id FROM events e JOIN articles a ON a.id=e.primary_article_id WHERE e.id=$1")
        .bind(event).fetch_one(store.pool()).await?;
    assert_eq!(primary, first);

    let mut state = EventState::Clustered;
    for next in [
        EventState::Verified,
        EventState::Drafted,
        EventState::Checked,
        EventState::Rendered,
        EventState::Queued,
        EventState::Publishing,
        EventState::Published,
    ] {
        store.transition(event, state, next, None, None).await?;
        state = next;
    }
    let update = make("update", "Rust anuncia versão 1.101")?;
    store
        .ingest_batch(first, &[update], Some("v3"), None, false, Utc::now())
        .await?;
    let job = store
        .claim_kind(lease, 5, "cluster")
        .await?
        .ok_or("update ausente")?;
    let child = store.cluster_job(&job, 0.86, 6).await?;
    assert_ne!(child, event);
    let parent: Option<i64> = sqlx::query_scalar("SELECT parent_event_id FROM events WHERE id=$1")
        .bind(child)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(parent, Some(event));
    let other = make("other", "Rust corrige vulnerabilidade de memória")?;
    store
        .ingest_batch(first, &[other], None, None, false, Utc::now())
        .await?;
    let job = store
        .claim_kind(lease, 5, "cluster")
        .await?
        .ok_or("outro fato ausente")?;
    let other_event = store.cluster_job(&job, 0.86, 6).await?;
    assert_ne!(other_event, event);
    assert_ne!(other_event, child);

    let burst = (0..200)
        .map(|i| make(&format!("burst/{i}"), "Rust anuncia versão 1.100"))
        .collect::<Result<Vec<_>, _>>()?;
    let started = std::time::Instant::now();
    let stats = store
        .ingest_batch(first, &burst, Some("v4"), None, false, Utc::now())
        .await?;
    assert_eq!(stats.inserted, 200);
    let again = store
        .ingest_batch(first, &burst, Some("v4"), None, false, Utc::now())
        .await?;
    assert_eq!(again.duplicates, 200);
    let mut processed = 0;
    let mut cluster_times = Vec::with_capacity(200);
    while let Some(job) = store.claim_kind(lease, 5, "cluster").await? {
        let cluster_started = std::time::Instant::now();
        assert_eq!(store.cluster_job(&job, 0.86, 6).await?, event);
        cluster_times.push(cluster_started.elapsed().as_secs_f64() * 1000.0);
        processed += 1;
    }
    assert_eq!(processed, 200);
    let burst_seconds = started.elapsed().as_secs_f64();
    assert!(burst_seconds < 60.0);
    cluster_times.sort_by(f64::total_cmp);
    let p95 = cluster_times[189];
    let report = serde_json::json!({"scope":"200 artigos normalizados: ingestão, deduplicação e clusterização transacional com Postgres real; sem feeds públicos ou LLM","profile":if cfg!(debug_assertions){"debug"}else{"release"},"items":processed,"burst_seconds":burst_seconds,"cluster_p50_ms":cluster_times[99],"cluster_p95_ms":p95,"cluster_max_ms":cluster_times[199],"target_cluster_p95_ms":50,"passed":p95<50.0});
    if let Some(path) = std::env::var_os("STORIES_CLUSTER_BENCHMARK") {
        let path = std::path::Path::new(&path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, serde_json::to_vec_pretty(&report)?).await?;
        assert!(p95 < 50.0, "clusterização excedeu p95 de 50 ms");
    }
    println!("{report}");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(count, 3);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM articles")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(count, 204);
    store
        .ingest_batch(first, &[], None, None, true, Utc::now())
        .await?;
    assert_eq!(store.source_state(first).await?.etag.as_deref(), Some("v4"));
    store
        .source_failed(first, "http", Utc::now() + chrono::Duration::seconds(120))
        .await?;
    assert_eq!(store.source_state(first).await?.failures, 1);
    store.set_paused(true).await?;
    assert!(store.claim_kind(lease, 5, "verify").await?.is_none());
    store.set_paused(false).await?;
    let pending = store
        .claim_kind(lease, 5, "verify")
        .await?
        .ok_or("pausa não liberou o job")?;
    store.complete(&pending).await?;
    store.close().await;
    Ok(())
}
