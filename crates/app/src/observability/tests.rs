use super::*;
use stories_config::Settings;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};
fn secret(value: &str) -> Result<Secret> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let settings = Settings::load(
        &root.join("config/exemplo.toml"),
        &root.join("config/sources.toml"),
    )?;
    Ok(settings
        .resolve_with(|name| match name {
            "DATABASE_URL" => Some(value.into()),
            "ADMIN_TOKEN" => Some("a".repeat(32)),
            _ => None,
        })?
        .database_url)
}
#[tokio::test]
async fn telegram_contract_limits_retries_and_rejects_remote_error_bodies() -> Result<()> {
    let http = stories_collectors::http::HttpClient::shared("BreakingBytesTests/0.1", 1024, 1)?
        .trusted_api_client();
    let mut notifier = Notifier::new(
        http.clone(),
        secret("123:contract_only_credential")?,
        secret("-42")?,
    )?;
    assert!(Notifier::new(http.clone(), secret("123:../../private")?, secret("-42")?).is_err());
    assert!(
        Notifier::new(
            http,
            secret("123:contract_only_credential")?,
            secret("@arbitrary")?
        )
        .is_err()
    );
    let server = MockServer::start().await;
    notifier.endpoint = format!("{}/sendMessage", server.uri());
    Mock::given(method("POST"))
        .and(path("/sendMessage"))
        .and(body_partial_json(
            json!({"chat_id":"-42","protect_content":true}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"message_id":1}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(notifier.send("unsupported_text", 99).await.is_ok());
    let requests = server
        .received_requests()
        .await
        .ok_or_else(|| anyhow::anyhow!("requisições ausentes"))?;
    assert!(!String::from_utf8_lossy(&requests[0].body).contains("contract_only_credential"));
    for (status, body, retry) in [
        (
            429,
            json!({"ok":false,"parameters":{"retry_after":900000},"description":"secret-from-provider"}),
            86400,
        ),
        (
            403,
            json!({"ok":false,"description":"secret-from-provider"}),
            3600,
        ),
        (
            200,
            json!({"ok":false,"description":"secret-from-provider"}),
            60,
        ),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&server)
            .await;
        let error = notifier
            .send("source_down", 1)
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("falha esperada"))?;
        assert_eq!(error.retry_s, retry);
        assert!(!format!("{error:?}").contains("secret-from-provider"));
    }
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(20_000)))
        .mount(&server)
        .await;
    assert!(notifier.send("source_down", 1).await.is_err());
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok":true}))
                .set_delay(Duration::from_secs(11)),
        )
        .mount(&server)
        .await;
    assert!(notifier.send("source_down", 1).await.is_err());
    Ok(())
}
#[test]
fn histogram_export_has_cumulative_infinity_sum_count_and_controlled_labels() {
    let snapshot = OperationalSnapshot {
        paused: false,
        articles: 0,
        queue_depth: 2,
        dead_jobs: 0,
        daily_budget_remaining: 9,
        no_news: false,
        latency_high: false,
        publication_failures: false,
        backup_stale: false,
        restore_stale: false,
        sources: vec![stories_store::SourceHealth {
            id: 1,
            up: true,
            silence_seconds: 0.0,
        }],
        metrics: vec![stories_store::MetricRow {
            name: "detect_to_publish_seconds".into(),
            label: "A:1".into(),
            total: 600.0,
            samples: 2,
            buckets: vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 2, 2],
        }],
    };
    let output = prometheus(&snapshot, Some(6), true);
    assert!(
        output.contains("detect_to_publish_seconds_bucket{layer=\"A\",level=\"1\",le=\"+Inf\"} 2")
    );
    assert!(output.contains("detect_to_publish_seconds_sum{layer=\"A\",level=\"1\"} 600"));
    assert!(output.contains("ig_token_days_left 6"));
    assert!(output.contains("source_up{source=\"1\"} 1"));
    assert!(!prometheus(&snapshot, None, false).contains("ig_token_days_left"));
}

#[test]
fn configured_metrics_exporter_records_runtime_histograms_without_global_recorder() -> Result<()> {
    initialize()?;
    runtime_observation(Metric::LlmCheck, 0.05);
    runtime_observation(Metric::Render, 0.10);
    let recorder = recorder().ok_or_else(|| anyhow::anyhow!("exportador ausente"))?;
    recorder.handle().run_upkeep();
    let text = recorder.handle().render();
    assert!(text.contains("llm_latency_seconds_count{stage=\"check\"}"));
    assert!(text.contains("render_duration_seconds_bucket{stage=\"render\",le=\"+Inf\"}"));
    Ok(())
}
