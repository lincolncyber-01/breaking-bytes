#![cfg(feature = "test-support")]
//! SIGKILL real após o efeito simulado, com Postgres persistente e novo processo.
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use stories_core::policy::ResolutionRequest;
use stories_store::{ResolutionWitness, Store};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use wiremock::{
    Mock, MockServer, Request, Respond, ResponseTemplate,
    matchers::{method, path},
};

struct Publish(Arc<AtomicUsize>);
impl Respond for Publish {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.0.fetch_add(1, Ordering::SeqCst);
        ResponseTemplate::new(200)
            .set_body_json(json!({"id":"999"}))
            .set_delay(Duration::from_secs(30))
    }
}
struct Status(Arc<AtomicUsize>);
impl Respond for Status {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(
            json!({"status_code":if self.0.load(Ordering::SeqCst)>0{"PUBLISHED"}else{"FINISHED"}}),
        )
    }
}
struct Stories(Arc<AtomicUsize>);
impl Respond for Stories {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(if self.0.load(Ordering::SeqCst) > 0 {
            json!({"data":[{"id":"999"}]})
        } else {
            json!({"data":[]})
        })
    }
}
#[tokio::test]
async fn sigkill_after_remote_effect_is_reconciled_without_a_second_post() -> Result<()> {
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
    let server = MockServer::start().await;
    let effects = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/debug_token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"app_id":"99","type":"USER","is_valid":true,"expires_at":(Utc::now()+chrono::Duration::days(30)).timestamp(),"scopes":["instagram_basic","instagram_content_publish","pages_read_engagement"]}}))).mount(&server).await;
    Mock::given(path("/v27.0/123/content_publishing_limit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[{"quota_usage":0,"config":{"quota_total":100,"quota_duration":86400}}]}),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/v27.0/88"))
        .respond_with(Status(effects.clone()))
        .mount(&server)
        .await;
    Mock::given(path("/v27.0/123/stories"))
        .respond_with(Stories(effects.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v27.0/123/media_publish"))
        .respond_with(Publish(effects.clone()))
        .mount(&server)
        .await;
    let http = stories_collectors::http::HttpClient::shared("BreakingBytesChaos/0.1", 1024, 1)?
        .trusted_api_client();
    let graph =
        stories_publisher::graph::Graph::test_server(http, &format!("{}/v27.0/", server.uri()))?;
    let account = graph.account_key();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temporary = std::env::temp_dir().join(format!("stories-sigkill-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&temporary).await?;
    let bytes = include_bytes!("../../../docs/reports/examples/story.jpg");
    tokio::fs::write(temporary.join("story.jpg"), bytes).await?;
    let event = store.create_event(None).await?;
    let source = store
        .register_source(
            "Chaos Source",
            "A",
            "rss",
            "https://example.com/feed",
            5,
            60,
        )
        .await?;
    let article:i64=sqlx::query_scalar("INSERT INTO articles(source_id,url_canonical,title,title_hash,published_at) VALUES($1,'https://example.com/chaos','Rust',decode(repeat('00',32),'hex'),now()) RETURNING id").bind(source).fetch_one(store.pool()).await?;
    sqlx::query("INSERT INTO event_articles(event_id,article_id) VALUES($1,$2)")
        .bind(event)
        .bind(article)
        .execute(store.pool())
        .await?;
    sqlx::query(
        "UPDATE events SET state='QUEUED',verified_revision=content_revision,score=0.9 WHERE id=$1",
    )
    .bind(event)
    .execute(store.pool())
    .await?;
    let revision: i64 = sqlx::query_scalar("SELECT content_revision FROM events WHERE id=$1")
        .bind(event)
        .fetch_one(store.pool())
        .await?;
    let hash = blake3::hash(bytes);
    sqlx::query("INSERT INTO rendered_stories(event_id,revision,template,artifact_path,content_hash,byte_length,width,height,layout,image_provenance,dry_run) VALUES($1,$2,'story-v1','story.jpg',$3,$4,1080,1920,'{}','{}',false)").bind(event).bind(revision).bind(hash.as_bytes().as_slice()).bind(bytes.len() as i64).execute(store.pool()).await?;
    let publication:i64=sqlx::query_scalar("INSERT INTO publications(event_id,kind,status,dry_run,account_key,revision,artifact_hash,container_id,image_key,container_created_at) VALUES($1,'new','container_ready',false,$2,$3,$4,'88','stories/chaos.jpg',now()) RETURNING id").bind(event).bind(&account).bind(revision).bind(hash.as_bytes().as_slice()).fetch_one(store.pool()).await?;
    store
        .enqueue(
            stories_store::Enqueue {
                kind: "publish",
                key: &format!("publish:{publication}"),
                payload: &json!({"publication_id":publication}),
            },
            Utc::now(),
        )
        .await?;
    let command = || {
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_stories-publication-chaos"));
        cmd.current_dir(&root)
            .env("STORIES_CHAOS_DATABASE", &url)
            .env("STORIES_CHAOS_GRAPH", format!("{}/", server.uri()))
            .env("STORIES_CHAOS_ARTIFACTS", &temporary)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        cmd
    };
    let mut first = command().spawn()?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while effects.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("efeito remoto não foi observado")?;
    assert!(
        store
            .publication(publication)
            .await?
            .request_started_at
            .is_some()
    );
    first.kill().await?;
    assert!(!first.wait().await?.success());
    let before = store.publication(publication).await?;
    assert_eq!(before.status, "publishing");
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second',run_at=now() WHERE payload->>'publication_id'=$1").bind(publication.to_string()).execute(store.pool()).await?;
    let mut second = command().spawn()?;
    let status = tokio::time::timeout(Duration::from_secs(15), second.wait()).await??;
    assert!(status.success());
    let recovered = store.publication(publication).await?;
    assert_eq!(recovered.status, "published_unresolved");
    assert!(recovered.media_id.is_none());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    store.set_paused(true).await?;
    let request = ResolutionRequest {
        request_id: uuid::Uuid::new_v4(),
        expected_status: "published_unresolved".into(),
        expected_container_id: "88".into(),
        media_id: "999".into(),
        attest_creation_mapping: true,
    };
    store
        .resolve_publication(
            publication,
            &request,
            ResolutionWitness {
                account,
                container_id: "88".into(),
                media_id: "999".into(),
                observed_at: Utc::now(),
                container_published: true,
                story_visible: true,
            },
        )
        .await?;
    assert_eq!(
        store.publication(publication).await?.media_id.as_deref(),
        Some("999")
    );
    let posts = server
        .received_requests()
        .await
        .context("requisições ausentes")?
        .into_iter()
        .filter(|r| r.method.as_str() == "POST")
        .count();
    assert_eq!(posts, 1);
    store.close().await;
    tokio::fs::remove_dir_all(temporary).await?;
    Ok(())
}
