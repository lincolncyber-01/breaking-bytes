use chrono::Utc;
use serde_json::json;
use std::time::Duration;
use stories_core::{EventState, PublicationKey, PublicationKind};
use stories_store::{Enqueue, JobFailure, Store, StoreError, retry_delay};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

#[tokio::test]
async fn postgres_outbox_leases_and_publication_uniqueness()
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
    store.migrate().await?;
    store.health().await?;
    let vector: String =
        sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname = 'vector'")
            .fetch_one(store.pool())
            .await?;
    assert_eq!(vector, "0.8.2");
    let payload = json!({"event_id": 1});
    let event = store
        .create_event(Some(Enqueue {
            kind: "normalize",
            key: "initial",
            payload: &payload,
        }))
        .await?;
    assert!(
        store
            .create_event(Some(Enqueue {
                kind: "normalize",
                key: "initial",
                payload: &payload
            }))
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(
        count, 1,
        "falha do outbox deve desfazer a criação do evento"
    );
    store
        .transition(
            event,
            EventState::Detected,
            EventState::Clustered,
            None,
            Some(Enqueue {
                kind: "verify",
                key: "verify-1",
                payload: &payload,
            }),
        )
        .await?;
    assert!(matches!(
        store
            .transition(
                event,
                EventState::Detected,
                EventState::Clustered,
                None,
                None
            )
            .await,
        Err(StoreError::Conflict)
    ));
    assert!(
        sqlx::query("UPDATE events SET state = 'PUBLISHED' WHERE id = $1")
            .bind(event)
            .execute(store.pool())
            .await
            .is_err()
    );
    let lease = Duration::from_secs(120);
    let (left, right) = tokio::join!(store.claim(lease, 5), store.claim(lease, 5));
    let left = left?.ok_or("primeiro job ausente")?;
    let right = right?.ok_or("segundo job ausente")?;
    assert_ne!(
        left.id, right.id,
        "claims concorrentes não podem compartilhar job"
    );
    store.complete(&right).await?;
    assert!(store.claim(lease, 5).await?.is_none());
    sqlx::query("UPDATE jobs SET lease_until = now() - interval '1 second' WHERE id = $1")
        .bind(left.id)
        .execute(store.pool())
        .await?;
    assert!(matches!(
        store.complete(&left).await,
        Err(StoreError::LostLease)
    ));
    let recovered = store.claim(lease, 5).await?.ok_or("lease não recuperado")?;
    assert_eq!(recovered.id, left.id);
    assert_ne!(recovered.lease_token, left.lease_token);
    assert_eq!(recovered.attempts, 2);
    assert!(matches!(
        store.renew(&left, lease).await,
        Err(StoreError::LostLease)
    ));
    assert!(matches!(
        store.retry(&left, JobFailure::Transient, 5).await,
        Err(StoreError::LostLease)
    ));
    store.renew(&recovered, lease).await?;
    store.retry(&recovered, JobFailure::Transient, 2).await?;
    let dead: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE dead_at IS NOT NULL")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(dead, 1);
    assert!(
        store
            .enqueue(
                Enqueue {
                    kind: "normalize",
                    key: "retry",
                    payload: &payload
                },
                Utc::now()
            )
            .await?
    );
    assert!(
        !store
            .enqueue(
                Enqueue {
                    kind: "normalize",
                    key: "retry",
                    payload: &payload
                },
                Utc::now()
            )
            .await?
    );
    let job = store.claim(lease, 5).await?.ok_or("job de retry ausente")?;
    store.retry(&job, JobFailure::Timeout, 5).await?;
    assert!(
        store.claim(lease, 5).await?.is_none(),
        "backoff deve adiar o job"
    );
    sqlx::query("UPDATE jobs SET run_at = now() WHERE id = $1")
        .bind(job.id)
        .execute(store.pool())
        .await?;
    let job = store.claim(lease, 5).await?.ok_or("job adiado ausente")?;
    store.complete(&job).await?;
    let key = PublicationKey::new(event, PublicationKind::New, 0)?;
    let (a, b) = tokio::join!(
        store.reserve_publication(key, true),
        store.reserve_publication(key, true)
    );
    let a = a?;
    let b = b?;
    assert_ne!(a.is_some(), b.is_some());
    store.set_paused(true).await?;
    assert!(store.paused().await?);
    store.set_paused(false).await?;
    assert!(!store.paused().await?);
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE event_id = $1")
        .bind(event)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(audit, 2);
    store.close().await;
    Ok(())
}

#[test]
fn exponential_backoff_has_bounded_jitter() {
    for attempt in -10..=100 {
        for seed in [0, 1, 7, u64::MAX] {
            assert!((2..=900).contains(&retry_delay(attempt, seed)));
        }
    }
    assert!(retry_delay(1, 0) < retry_delay(8, 0));
}
