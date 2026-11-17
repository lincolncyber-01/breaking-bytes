//! Contratos reais de histogramas, contadores, quotas, alertas e quarentena de restauração.
use proptest::{prelude::*, strategy::ValueTree};
use std::time::Duration;
use stories_store::{AlertCondition, Metric, Store, StoreError};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
#[tokio::test]
async fn telemetry_is_durable_atomic_bounded_and_alert_leases_are_fenced()
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
    store
        .provision_roles(&"a".repeat(32), &"b".repeat(32), &"c".repeat(32))
        .await?;
    let owner_url = url.replace(
        "postgres:integration-only@",
        &format!("stories_owner:{}@", "a".repeat(32)),
    );
    let owner = Store::connect(&owner_url, 2, Duration::from_secs(10)).await?;
    owner.migrate().await?;
    owner.close().await;
    let source = store
        .register_source("Telemetry", "A", "rss", "https://example.com/feed", 5, 60)
        .await?;
    sqlx::query("INSERT INTO articles(source_id,url_canonical,title,title_hash,published_at) VALUES($1,'https://example.com/news','Rust',decode(repeat('00',32),'hex'),now()-interval '1 minute')").bind(source).execute(store.pool()).await?;
    let snapshot = store.operational_snapshot(true, 40).await?;
    assert_eq!(snapshot.articles, 1);
    assert_eq!(snapshot.daily_budget_remaining, 40);
    assert_eq!(
        store
            .operational_snapshot(false, 40)
            .await?
            .daily_budget_remaining,
        0
    );
    store.record_quota("contract", 5, 100).await?;
    assert_eq!(
        store
            .operational_snapshot(false, 40)
            .await?
            .daily_budget_remaining,
        35
    );
    assert_eq!(
        snapshot
            .metrics
            .iter()
            .find(|m| m.name == "articles_ingested_total")
            .ok_or("contador ausente")?
            .total,
        1.0
    );
    assert!(!snapshot.sources[0].up);
    assert!(snapshot.sources[0].silence_seconds < 600.0);
    assert!(matches!(
        store.metric(Metric::Render, f64::NAN).await,
        Err(StoreError::InvalidOptions)
    ));
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let samples = proptest::collection::vec(0.0f64..10_000.0, 64)
        .new_tree(&mut runner)
        .map_err(|_| "geração de amostras falhou")?
        .current();
    for sample in &samples {
        store.metric(Metric::Render, *sample).await?;
    }
    let snapshot = store.operational_snapshot(true, 40).await?;
    let histogram = snapshot
        .metrics
        .iter()
        .find(|m| m.name == "render_duration_seconds")
        .ok_or("histograma ausente")?;
    assert_eq!(histogram.samples, 64);
    assert!(histogram.buckets.windows(2).all(|pair| pair[0] <= pair[1]));
    assert_eq!(histogram.buckets.last(), Some(&64));
    assert!((histogram.total - samples.iter().sum::<f64>()).abs() < 0.000001);
    store.metric(Metric::Instagram(u32::MAX), 1.0).await?;
    assert_eq!(
        store
            .operational_snapshot(true, 40)
            .await?
            .metrics
            .iter()
            .find(|m| m.name == "ig_api_errors_total")
            .ok_or("série ausente")?
            .label,
        "other"
    );
    let conditions = [AlertCondition {
        code: "source_down",
        subject: i64::from(source),
    }];
    store.sync_alerts(&conditions).await?;
    let old = store.claim_alert().await?.ok_or("lease ausente")?;
    assert!(store.claim_alert().await?.is_none());
    sqlx::query("UPDATE operational_alerts SET lease_until=now()-interval '1 second'")
        .execute(store.pool())
        .await?;
    let new = store.claim_alert().await?.ok_or("lease não recuperado")?;
    assert_ne!(old.lease_token, new.lease_token);
    assert!(matches!(
        store.finish_alert(&old, true, 3600).await,
        Err(StoreError::LostLease)
    ));
    store.finish_alert(&new, false, 120).await?;
    store.sync_alerts(&conditions).await?;
    assert!(
        store.claim_alert().await?.is_none(),
        "sincronização não pode apagar Retry-After"
    );
    sqlx::query("UPDATE operational_alerts SET next_attempt=now()")
        .execute(store.pool())
        .await?;
    let lease = store.claim_alert().await?.ok_or("tentativa ausente")?;
    store.finish_alert(&lease, true, 3600).await?;
    store.sync_alerts(&[]).await?;
    let active: bool =
        sqlx::query_scalar("SELECT active FROM operational_alerts WHERE code='source_down'")
            .fetch_one(store.pool())
            .await?;
    assert!(!active);
    let event = store.create_event(None).await?;
    sqlx::query("INSERT INTO audit_log(event_id,step,detail) VALUES($1,'rewrite_requested','{\"code\":\"unsupported_claim\"}')").bind(event).execute(store.pool()).await?;
    let quality = store
        .claim_alert()
        .await?
        .ok_or("bloqueio factual sem alerta")?;
    assert_eq!(quality.code, "unsupported_text");
    assert_eq!(quality.subject, event);
    store.finish_alert(&quality, true, 3600).await?;
    sqlx::query("UPDATE operational_alerts SET last_sent=now()-interval '2 hours',next_attempt=now() WHERE code='unsupported_text'").execute(store.pool()).await?;
    assert!(
        store.claim_alert().await?.is_none(),
        "bloqueio factual individual não repete a cada hora"
    );
    sqlx::query(
        "UPDATE sources SET monitoring_since=now()-interval '3 hours',last_success_at=NULL",
    )
    .execute(store.pool())
    .await?;
    assert!(store.operational_snapshot(true, 40).await?.sources[0].silence_seconds > 600.0);
    sqlx::query(
        "UPDATE pipeline_control SET paused=true,restore_hold_until=now()+interval '24 hours'",
    )
    .execute(store.pool())
    .await?;
    assert!(store.set_paused(false).await.is_err());
    assert!(store.paused().await?);
    sqlx::query("UPDATE pipeline_control SET restore_hold_until=now()-interval '1 second'")
        .execute(store.pool())
        .await?;
    store.set_paused(false).await?;
    assert!(!store.runtime_role_safe().await?);
    store
        .provision_roles(&"a".repeat(32), &"b".repeat(32), &"c".repeat(32))
        .await?;
    store
        .provision_roles(&"a".repeat(32), &"b".repeat(32), &"c".repeat(32))
        .await?;
    let runtime_url = url.replace(
        "postgres:integration-only@",
        &format!("stories_runtime:{}@", "b".repeat(32)),
    );
    let runtime = Store::connect(&runtime_url, 2, Duration::from_secs(10)).await?;
    assert!(runtime.runtime_role_safe().await?);
    assert!(
        sqlx::query("CREATE TABLE blocked_runtime(id int)")
            .execute(runtime.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE audit_log SET step='tampered' WHERE false")
            .execute(runtime.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM publications WHERE false")
            .execute(runtime.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE pipeline_control SET restore_hold_until=NULL")
            .execute(runtime.pool())
            .await
            .is_err()
    );
    runtime.metric(Metric::Render, 0.1).await?;
    runtime.sync_alerts(&[]).await?;
    runtime.set_paused(true).await?;
    assert!(runtime.operational_snapshot(true, 40).await?.paused);
    runtime.close().await;
    store.close().await;
    Ok(())
}
