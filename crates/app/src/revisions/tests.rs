use super::*;

#[test]
fn watch_policy_uses_validated_configuration() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let settings = Settings::load(
        &root.join("config/exemplo.toml"),
        &root.join("config/sources.toml"),
    )?;
    let p = policy(&settings);
    assert_eq!(p.window_hours, 24);
    assert_eq!(p.high_impact_hours, 72);
    assert!(!settings.corrections.enabled);
    Ok(())
}

use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use stories_core::CapturedArticle;
use stories_store::WatchContext;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

struct Changed;
impl RevisionReader for Changed {
    async fn read(&self, e: &Evidence) -> Result<Option<CapturedRevision>, &'static str> {
        Ok(Some(CapturedRevision{article:CapturedArticle{url:e.article_url.clone(),title:"Rust corrige versão 1.101".into(),excerpt:"Rust corrige versão 1.101. Correção: Rust confirma versão 1.101. O anúncio anterior foi corrigido.".into(),published_at:e.published_at,image_url:None},notice:Some("Correção: Rust confirma versão 1.101. O anúncio anterior foi corrigido.".into())}))
    }
}
struct Malformed;
impl RevisionReader for Malformed {
    async fn read(&self, e: &Evidence) -> Result<Option<CapturedRevision>, &'static str> {
        let mut captured = Changed.read(e).await?;
        if let Some(c) = &mut captured {
            c.article.title.clear();
        }
        Ok(captured)
    }
}
struct Missing;
impl RevisionReader for Missing {
    async fn read(&self, _: &Evidence) -> Result<Option<CapturedRevision>, &'static str> {
        Ok(None)
    }
}
struct Forbidden;
impl crate::editorial::EvidenceClient for Forbidden {
    async fn hydrate(&self, _: Vec<Evidence>) -> Vec<Evidence> {
        Vec::new()
    }
}
struct ReviewClient {
    calls: AtomicUsize,
}
fn verdict(data: &Value) -> Result<Value, LlmError> {
    let source = data["evidence"]["sources"].get(0).ok_or(LlmError::Input)?;
    let previous = data["previous_sources"].get(0).ok_or(LlmError::Input)?;
    Ok(
        json!({"decision":"correction","citations":[{"article_id":source["article_id"],"source_id":source["source_id"],"published_segment":0,"before_quote":previous["title"],"after_quote":"Rust confirma versão 1.101","notice_quote":"Correção: Rust confirma versão 1.101."}]}),
    )
}
impl CompletionClient for ReviewClient {
    async fn complete(&self, task: Task, data: Value) -> Result<Value, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match task {
            Task::ReviewChange => verdict(&data),
            Task::Draft => Ok(
                json!({"title":"Rust corrige versão 1.101","summary":["Rust confirma versão 1.101.","O anúncio anterior foi corrigido."],"source":"Rust Blog","kind":"correction","entities":["Rust"],"language":"pt-BR"}),
            ),
            Task::CheckRevision => {
                let source = &data["evidence"]["sources"][0];
                let segments=data["segments"].as_array().ok_or(LlmError::Input)?.iter().enumerate().map(|(index,text)|json!({"index":index,"text":text,"supported":true,"claims":[{"text":text,"supported":true,"evidence":[{"article_id":source["article_id"],"quote":source["excerpt"]}]}]})).collect::<Vec<_>>();
                Ok(
                    json!({"report":{"language_ok":true,"neutral":true,"segments":segments},"review":verdict(&data["change_context"])?}),
                )
            }
            Task::Check => Err(LlmError::Input),
        }
    }
}
async fn claim(store: &Store, kind: &str, field: &str, id: i64) -> Result<Job> {
    Ok(sqlx::query_as("UPDATE jobs SET lease_token=$4,lease_until=now()+interval '120 seconds',attempts=attempts+1 WHERE id=(SELECT id FROM jobs WHERE kind=$1 AND payload->>$2=$3 AND done_at IS NULL AND dead_at IS NULL AND (lease_until IS NULL OR lease_until<=now()) ORDER BY id LIMIT 1) RETURNING id,kind,payload,attempts,lease_token,lease_until")
        .bind(kind).bind(field).bind(id.to_string()).bind(uuid::Uuid::new_v4()).fetch_one(store.pool()).await?)
}
async fn force_observation(
    store: &Store,
    settings: &Settings,
    id: i64,
    reader: &impl RevisionReader,
) -> Result<WatchContext> {
    let job = claim(store, "revisit", "publication_id", id).await?;
    observe(settings, store, &job, reader).await?;
    Ok(store.watch_context(id).await?)
}

#[tokio::test]
async fn correction_dry_run_survives_crash_and_fences_changed_sources() -> Result<()> {
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
        std::env::temp_dir().join(format!("stories-revisions-{}", uuid::Uuid::new_v4()));
    settings.imaging.artifacts_dir = temporary.to_string_lossy().into_owned();
    let engine = Arc::new(Editorial::new()?);
    let client = ReviewClient {
        calls: AtomicUsize::new(0),
    };
    let first = crate::imaging::tests::seed(&store, source, 101, false).await?;
    crate::imaging::tests::render_fixture(&settings, &store, first).await?;
    let original = crate::publication::tests::dry_fixture(&settings, &store, first).await?;
    // Simula passagem de vinte horas na fixture sem trocar o conteúdo do Story.
    sqlx::query("UPDATE articles SET published_at=now()-interval '20 hours' WHERE id IN(SELECT article_id FROM event_articles WHERE event_id=$1)").bind(first).execute(store.pool()).await?;
    sqlx::query("UPDATE publications SET published_at=now()-interval '20 hours',editorial_snapshot=jsonb_set(editorial_snapshot,'{assessment,sources,0,published_at}',to_jsonb(now()-interval '20 hours')) WHERE id=$1").bind(original).execute(store.pool()).await?;
    assert_eq!(store.enroll_watches(policy(&settings), true).await?, 1);
    assert_eq!(store.enroll_watches(policy(&settings), true).await?, 0);
    let mut real_mode = settings.clone();
    real_mode.instagram.dry_run = false;
    let untouched = force_observation(&store, &real_mode, original, &Changed).await?;
    assert_eq!(untouched.version, 0);
    let observed:i64=sqlx::query_scalar("SELECT count(*) FROM publication_source_snapshots WHERE publication_id=$1 AND fetched_at IS NOT NULL").bind(original).fetch_one(store.pool()).await?;
    assert_eq!(observed, 0);
    let frozen: Value =
        sqlx::query_scalar("SELECT editorial_snapshot FROM publications WHERE id=$1")
            .bind(original)
            .fetch_one(store.pool())
            .await?;
    let missing = force_observation(&store, &settings, original, &Missing).await?;
    assert_eq!(missing.version, 1);
    let (count,failures):(i64,i32)=sqlx::query_as("SELECT (SELECT count(*) FROM publication_reviews),failures FROM publication_source_snapshots WHERE publication_id=$1").bind(original).fetch_one(store.pool()).await?;
    assert_eq!(count, 0);
    assert_eq!(failures, 1);
    let malformed = force_observation(&store, &settings, original, &Malformed).await?;
    assert_eq!(malformed.version, missing.version);
    let code: String = sqlx::query_scalar(
        "SELECT last_error_code FROM publication_source_snapshots WHERE publication_id=$1",
    )
    .bind(original)
    .fetch_one(store.pool())
    .await?;
    assert_eq!(code, "parse");
    let changed = force_observation(&store, &settings, original, &Changed).await?;
    assert_eq!(changed.version, 2);
    let again = force_observation(&store, &settings, original, &Changed).await?;
    assert_eq!(again.version, changed.version);
    let review_id: i64 =
        sqlx::query_scalar("SELECT id FROM publication_reviews WHERE publication_id=$1")
            .bind(original)
            .fetch_one(store.pool())
            .await?;
    let stale = claim(&store, "review_revision", "review_id", review_id).await?;
    let record = store.review_record(review_id).await?;
    let a = record.context.assessment(Utc::now(), 6);
    let decision = engine.parse_change(
        verdict(&record.context.input(&a))?,
        &record.context,
        &a,
        Utc::now(),
        6,
    )?;
    sqlx::query("UPDATE jobs SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(stale.id)
        .execute(store.pool())
        .await?;
    assert!(matches!(
        store
            .apply_revision_review(&stale, &record, decision, 6)
            .await,
        Err(StoreError::LostLease)
    ));
    let recovered = claim(&store, "review_revision", "review_id", review_id).await?;
    review_one(&settings, &store, &recovered, &client, engine.clone()).await?;
    let child: i64 =
        sqlx::query_scalar("SELECT candidate_event_id FROM publication_reviews WHERE id=$1")
            .bind(review_id)
            .fetch_one(store.pool())
            .await?;
    assert!(store.revision_fresh(child, 6, true).await?);
    assert!(!store.revision_fresh(child, 6, false).await?);
    // A decisão e a outbox já estão persistidas; repetir o lease antigo não cria evento.
    assert!(
        store
            .apply_revision_review(
                &recovered,
                &record,
                ChangeReview {
                    decision: ChangeDecision::Uncertain,
                    citations: vec![]
                },
                6
            )
            .await
            .is_err()
    );
    let verification = crate::imaging::tests::claim(&store, child, "verify").await?;
    crate::editorial::process(
        &settings,
        &store,
        &verification,
        &client,
        &Forbidden,
        engine.clone(),
    )
    .await?;
    assert_eq!(
        store.editorial_context(child).await?.state,
        stories_core::EventState::Checked
    );
    crate::imaging::tests::render_fixture(&settings, &store, child).await?;
    // Uma revisão irmã em estado incerto bloqueia outra reserva da mesma publicação.
    let sibling:i64=sqlx::query_scalar("INSERT INTO events(parent_event_id,review_id,publication_kind) VALUES($1,$2,'correction') RETURNING id").bind(first).bind(review_id).fetch_one(store.pool()).await?;
    let uncertain:i64=sqlx::query_scalar("INSERT INTO publications(event_id,kind,status,dry_run,account_key) VALUES($1,'correction','uncertain',true,'dry_run-local') RETURNING id").bind(sibling).fetch_one(store.pool()).await?;
    let schedule = crate::imaging::tests::claim(&store, child, "schedule").await?;
    store
        .schedule_publication(
            &schedule,
            child,
            store.editorial_context(child).await?.revision,
            0.1,
            json!({"test":true}),
            stories_store::PublicationPolicy {
                account: "dry_run-local",
                dry_run: true,
                policy: stories_scheduler::Policy {
                    daily_cap: 40,
                    reserved: 5,
                    breaking_score: 0.8,
                    min_gap_seconds: 90,
                },
                remote_usage: 0,
                remote_limit: 100,
                max_age_hours: 6,
            },
        )
        .await?;
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM publications WHERE event_id=$1")
        .bind(child)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(pending, 0);
    sqlx::query("UPDATE publications SET status='failed' WHERE id=$1")
        .bind(uncertain)
        .execute(store.pool())
        .await?;
    let correction = crate::publication::tests::dry_fixture(&settings, &store, child).await?;
    let p = store.publication(correction).await?;
    assert_eq!(p.kind, "correction");
    assert!(p.container_id.is_none() && p.media_id.is_none());
    assert_eq!(client.calls.load(Ordering::SeqCst), 3);
    let old_snapshot: Value =
        sqlx::query_scalar("SELECT editorial_snapshot FROM publications WHERE id=$1")
            .bind(original)
            .fetch_one(store.pool())
            .await?;
    assert_eq!(old_snapshot, frozen);
    let parent_status: String = sqlx::query_scalar("SELECT state FROM events WHERE id=$1")
        .bind(first)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(parent_status, "QUEUED");
    assert!(!store.watch_context(original).await?.active);
    assert_eq!(store.enroll_watches(policy(&settings), true).await?, 1);
    let next = store.watch_context(correction).await?;
    assert_eq!(
        next.sources[0].baseline.published_at,
        record.context.changes[0].after.published_at
    );
    force_observation(&store, &settings, correction, &Changed).await?;
    let reviews: i64 =
        sqlx::query_scalar("SELECT count(*) FROM publication_reviews WHERE publication_id=$1")
            .bind(correction)
            .fetch_one(store.pool())
            .await?;
    assert_eq!(reviews, 0);

    // Avança o relógio da fixture para não depender do intervalo entre Stories normais.
    sqlx::query("UPDATE publications SET published_at=now()-interval '2 minutes',request_started_at=NULL WHERE status='dry_run'").execute(store.pool()).await?;
    // Outro Story tem resposta do LLM em voo quando a fonte fica indisponível.
    let second = crate::imaging::tests::seed(&store, source, 102, false).await?;
    crate::imaging::tests::render_fixture(&settings, &store, second).await?;
    let second_pub = crate::publication::tests::dry_fixture(&settings, &store, second).await?;
    store.enroll_watches(policy(&settings), true).await?;
    force_observation(&store, &settings, second_pub, &Changed).await?;
    let review_id: i64 =
        sqlx::query_scalar("SELECT id FROM publication_reviews WHERE publication_id=$1")
            .bind(second_pub)
            .fetch_one(store.pool())
            .await?;
    let job = claim(&store, "review_revision", "review_id", review_id).await?;
    let record = store.review_record(review_id).await?;
    let a = record.context.assessment(Utc::now(), 6);
    let decision = engine.parse_change(
        verdict(&record.context.input(&a))?,
        &record.context,
        &a,
        Utc::now(),
        6,
    )?;
    force_observation(&store, &settings, second_pub, &Missing).await?;
    assert!(
        store
            .apply_revision_review(&job, &record, decision, 6)
            .await?
            .is_none()
    );
    let status: String = sqlx::query_scalar("SELECT status FROM publication_reviews WHERE id=$1")
        .bind(review_id)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(status, "superseded");
    // A retomada de fonte cria outra revisão; mudança posterior bloqueia também o render.
    force_observation(&store, &settings, second_pub, &Changed).await?;
    let new_id: i64 =
        sqlx::query_scalar("SELECT max(id) FROM publication_reviews WHERE publication_id=$1")
            .bind(second_pub)
            .fetch_one(store.pool())
            .await?;
    let job = claim(&store, "review_revision", "review_id", new_id).await?;
    review_one(&settings, &store, &job, &client, engine.clone()).await?;
    let blocked: i64 =
        sqlx::query_scalar("SELECT candidate_event_id FROM publication_reviews WHERE id=$1")
            .bind(new_id)
            .fetch_one(store.pool())
            .await?;
    let verification = crate::imaging::tests::claim(&store, blocked, "verify").await?;
    crate::editorial::process(
        &settings,
        &store,
        &verification,
        &client,
        &Forbidden,
        engine,
    )
    .await?;
    force_observation(&store, &settings, second_pub, &Missing).await?;
    crate::imaging::tests::render_fixture(&settings, &store, blocked).await?;
    assert_eq!(
        store.editorial_context(blocked).await?.state,
        stories_core::EventState::Superseded
    );
    let rendered: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rendered_stories WHERE event_id=$1")
            .bind(blocked)
            .fetch_one(store.pool())
            .await?;
    assert_eq!(rendered, 0);
    store.close().await;
    tokio::fs::remove_dir_all(temporary).await?;
    Ok(())
}
