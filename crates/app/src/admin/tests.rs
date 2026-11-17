use super::*;
use anyhow::{Context, Result};
use axum::{body::Body, http::Request};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
use stories_config::Settings;
use stories_core::{
    EventState,
    policy::{ResolutionRequest, RuleKind},
    token::{TokenKind, TokenObservation, TokenSource},
};
use stories_store::{Enqueue, Store};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

fn request(method: &str, url: &str, body: Value) -> Result<Request<Body>> {
    Ok(Request::builder()
        .method(method)
        .uri(url)
        .header("authorization", format!("Bearer {}", "a".repeat(40)))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))?)
}
fn witness(account: &str) -> ResolutionWitness {
    ResolutionWitness {
        account: account.into(),
        container_id: "77".into(),
        media_id: "999".into(),
        observed_at: Utc::now(),
        container_published: true,
        story_visible: true,
    }
}
#[tokio::test]
async fn administrative_rules_and_resolution_are_authenticated_atomic_and_fenced() -> Result<()> {
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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let settings = Settings::load(
        &root.join("config/exemplo.toml"),
        &root.join("config/sources.toml"),
    )?;
    let secrets = settings.resolve_with(|name| match name {
        "DATABASE_URL" => Some(url.clone()),
        "ADMIN_TOKEN" => Some("a".repeat(40)),
        _ => None,
    })?;
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
    let event = crate::imaging::tests::seed(&store, source, 501, false).await?;
    assert!(!store.event_blocked(event).await?);
    let rule = RuleChange {
        kind: RuleKind::Term,
        value: "VERSA\u{303}O".into(),
        blocked: true,
    };
    assert!(store.change_rule(&rule).await.is_err());
    store.set_paused(true).await?;
    store.change_rule(&rule).await?;
    assert!(store.event_blocked(event).await?);
    let actual: String = sqlx::query_scalar("SELECT policy_text($1)")
        .bind(&rule.value)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(actual, stories_core::policy::text(&rule.value));
    let unblocked = RuleChange {
        blocked: false,
        ..rule
    };
    store.change_rule(&unblocked).await?;
    assert!(!store.event_blocked(event).await?);
    store
        .change_rule(&RuleChange {
            kind: RuleKind::Source,
            value: "rust blog".into(),
            blocked: true,
        })
        .await?;
    assert!(store.source_state(source).await?.blocked);
    assert!(store.editorial_context(event).await?.evidence.is_empty());
    store
        .change_rule(&RuleChange {
            kind: RuleKind::Source,
            value: "Rust Blog".into(),
            blocked: false,
        })
        .await?;
    store
        .change_rule(&RuleChange {
            kind: RuleKind::Topic,
            value: "correções".into(),
            blocked: true,
        })
        .await?;
    assert!(store.event_blocked(event).await?);
    store.set_paused(false).await?;
    let render = crate::imaging::tests::claim(&store, event, "render").await?;
    let ctx = store.editorial_context(event).await?;
    store
        .editorial_commit(
            &render,
            &ctx,
            stories_store::EditorialStep::Rejected {
                code: "policy_blocked".into(),
            },
        )
        .await?;
    assert_eq!(
        store.editorial_context(event).await?.state,
        EventState::Rejected
    );
    store.set_paused(true).await?;
    store
        .change_rule(&RuleChange {
            kind: RuleKind::Topic,
            value: "correções".into(),
            blocked: false,
        })
        .await?;

    let server = MockServer::start().await;
    Mock::given(path("/v27.0/77"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status_code":"PUBLISHED"})))
        .mount(&server)
        .await;
    Mock::given(path("/v27.0/123/stories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"999"}]})))
        .mount(&server)
        .await;
    let _ = stories_collectors::http::HttpClient::shared("BreakingBytes/0.1", 1024, 1)?;
    let graph = stories_publisher::graph::Graph::test_server(
        reqwest::Client::builder().no_proxy().build()?,
        &format!("{}/v27.0/", server.uri()),
    )?;
    let account = graph.account_key();
    let pending = store.create_event(None).await?;
    sqlx::query("UPDATE events SET state='PUBLISHING' WHERE id=$1")
        .bind(pending)
        .execute(store.pool())
        .await?;
    let publication:i64=sqlx::query_scalar("INSERT INTO publications(event_id,kind,status,dry_run,account_key,container_id,request_started_at) VALUES($1,'new','uncertain',false,$2,'77',now()) RETURNING id").bind(pending).bind(&account).fetch_one(store.pool()).await?;
    store
        .enqueue(
            Enqueue {
                kind: "reconcile",
                key: "admin:reconcile",
                payload: &json!({"publication_id":publication}),
            },
            Utc::now(),
        )
        .await?;
    let resolve = ResolutionRequest {
        request_id: Uuid::new_v4(),
        expected_status: "uncertain".into(),
        expected_container_id: "77".into(),
        media_id: "999".into(),
        attest_creation_mapping: true,
    };
    let active = store
        .claim_kind(std::time::Duration::from_secs(30), 5, "reconcile")
        .await?
        .context("job ausente")?;
    assert!(
        store
            .resolve_publication(publication, &resolve, witness(&account))
            .await
            .is_err()
    );
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(active.id)
        .execute(store.pool())
        .await?;
    let bridge = Arc::new(crate::publication::AdminPublisher::default());
    bridge.bind(graph).await;
    let token = Arc::new(crate::token::TokenState::default());
    token
        .test_install(TokenObservation {
            kind: TokenKind::Instagram,
            source: TokenSource::OAuthEnvironment,
            valid: true,
            app_id: None,
            profile_id: None,
            issued_at: Some(Utc::now() - Duration::days(1)),
            expires_at: Some(Utc::now() + Duration::days(30)),
            data_access_expires_at: None,
            checked_at: Utc::now(),
            refreshed_at: None,
            scopes: vec![
                "instagram_business_basic".into(),
                "instagram_business_content_publish".into(),
            ],
            rotation_required: false,
            failure: None,
        })
        .await;
    let app = crate::router(AppState {
        store: store.clone(),
        admin_token: secrets.admin_token,
        dry_run: false,
        collection_enabled: false,
        editorial_enabled: false,
        imaging_enabled: false,
        publishing_enabled: true,
        corrections_enabled: false,
        publishing_ready: Arc::new(AtomicBool::new(true)),
        publisher_admin: bridge,
        token_state: token,
    });
    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin/blocklist")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let listing = app
        .clone()
        .oneshot(request("GET", "/admin/blocklist", json!({}))?)
        .await?;
    assert_eq!(listing.status(), StatusCode::OK);
    let endpoint = format!("/admin/publications/{publication}/resolve");
    let response = app
        .clone()
        .oneshot(request("POST", &endpoint, serde_json::to_value(&resolve)?)?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let p = store.publication(publication).await?;
    assert_eq!(p.status, "published");
    assert_eq!(p.media_id.as_deref(), Some("999"));
    assert_eq!(
        store.editorial_context(pending).await?.state,
        EventState::Published
    );
    store.set_paused(false).await?;
    let replay = app
        .clone()
        .oneshot(request("POST", &endpoint, serde_json::to_value(&resolve)?)?)
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let mut changed = resolve.clone();
    changed.media_id = "998".into();
    assert!(
        store
            .resolution_receipt(publication, &changed)
            .await
            .is_err()
    );
    let audit:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE event_id=$1 AND step='publication_resolved_by_operator'").bind(pending).fetch_one(store.pool()).await?;
    assert_eq!(audit, 1);
    let requests = server
        .received_requests()
        .await
        .context("requisições ausentes")?;
    assert!(requests.iter().all(|r| r.method.as_str() == "GET"));
    let done: bool = sqlx::query_scalar("SELECT done_at IS NOT NULL FROM jobs WHERE id=$1")
        .bind(active.id)
        .fetch_one(store.pool())
        .await?;
    assert!(done);
    store.close().await;
    Ok(())
}
