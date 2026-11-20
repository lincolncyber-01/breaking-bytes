//! Exportação privada de métricas e alertas sem texto de terceiros, URL ou credencial nos logs.
use crate::{AppState, token::TokenState};
use anyhow::{Context, Result};
use axum::{
    Json,
    extract::State,
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusRecorder};
use serde_json::{Value, json};
use std::{
    fmt::Write,
    future::Future,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use stories_config::{Secret, Settings};
use stories_editorial::{
    Task,
    client::{CompletionClient, LlmError},
};
use stories_publisher::PublishError;
use stories_store::{AlertCondition, Metric, OperationalSnapshot, Store};
use tokio::sync::watch;

static STARTED: OnceLock<Instant> = OnceLock::new();
static RECORDER: OnceLock<Option<PrometheusRecorder>> = OnceLock::new();
fn recorder() -> Option<&'static PrometheusRecorder> {
    RECORDER
        .get_or_init(|| {
            PrometheusBuilder::new()
                .set_buckets(&[
                    0.005, 0.01, 0.05, 0.1, 0.15, 0.5, 1.0, 5.0, 30.0, 60.0, 300.0, 600.0, 3600.0,
                    86400.0,
                ])
                .ok()
                .map(|builder| builder.build_recorder())
        })
        .as_ref()
}
pub fn initialize() -> Result<()> {
    STARTED.get_or_init(Instant::now);
    recorder().context("não foi possível inicializar o exportador Prometheus")?;
    Ok(())
}
fn runtime_observation(metric: Metric, value: f64) {
    let series = match metric {
        Metric::LlmDraft => Some(("llm_latency_seconds", "draft")),
        Metric::LlmCheck => Some(("llm_latency_seconds", "check")),
        Metric::LlmReview => Some(("llm_latency_seconds", "review_change")),
        Metric::LlmCheckRevision => Some(("llm_latency_seconds", "check_revision")),
        Metric::Render => Some(("render_duration_seconds", "render")),
        Metric::Instagram(_) => None,
    };
    if let (Some(recorder), Some((name, stage))) = (recorder(), series) {
        // O escopo é síncrono e não atravessa await; o recorder não vaza entre tarefas.
        metrics::with_local_recorder(recorder, || {
            metrics::histogram!(name,"stage"=>stage).record(value)
        });
    }
}
fn uptime() -> f64 {
    STARTED.get_or_init(Instant::now).elapsed().as_secs_f64()
}

pub async fn record(store: &Store, metric: Metric, value: f64) {
    if !value.is_finite() || !(0.0..=31_536_000.0).contains(&value) {
        return;
    }
    runtime_observation(metric, value);
    if store.metric(metric, value).await.is_err() {
        tracing::warn!("não foi possível registrar a amostra operacional");
    }
}
pub async fn instagram<T>(
    store: &Store,
    work: impl Future<Output = std::result::Result<T, PublishError>>,
) -> std::result::Result<T, PublishError> {
    let result = work.await;
    if let Err(error) = &result {
        record(store, Metric::Instagram(error.code()), 1.0).await;
    }
    result
}
#[derive(Clone)]
pub struct ObservedClient<C> {
    client: C,
    store: Store,
}
impl<C> ObservedClient<C> {
    pub fn new(client: C, store: Store) -> Self {
        Self { client, store }
    }
}
impl<C: CompletionClient> CompletionClient for ObservedClient<C> {
    async fn complete(&self, task: Task, data: Value) -> std::result::Result<Value, LlmError> {
        let started = Instant::now();
        let result = self.client.complete(task, data).await;
        let metric = match task {
            Task::Draft => Metric::LlmDraft,
            Task::Check => Metric::LlmCheck,
            Task::ReviewChange => Metric::LlmReview,
            Task::CheckRevision => Metric::LlmCheckRevision,
        };
        record(&self.store, metric, started.elapsed().as_secs_f64()).await;
        result
    }
}
fn prometheus(
    snapshot: &OperationalSnapshot,
    token_days: Option<i64>,
    token_valid: bool,
) -> String {
    let mut out = String::with_capacity(16_384);
    let mut last = "";
    let limits = [
        "0.005", "0.01", "0.05", "0.1", "0.15", "0.5", "1", "5", "30", "60", "300", "600", "3600",
        "86400", "+Inf",
    ];
    for metric in &snapshot.metrics {
        if matches!(
            metric.name.as_str(),
            "llm_latency_seconds" | "render_duration_seconds"
        ) {
            continue;
        }
        let histogram = metric.name.ends_with("_seconds");
        if last != metric.name {
            let _ = writeln!(
                out,
                "# TYPE {} {}",
                metric.name,
                if histogram { "histogram" } else { "counter" }
            );
            last = &metric.name;
        }
        let labels = match metric.name.as_str() {
            "detect_latency_seconds" => format!("layer=\"{}\"", metric.label),
            "detect_to_publish_seconds" => {
                let (layer, level) = metric.label.split_once(':').unwrap_or(("D", "0"));
                format!("layer=\"{layer}\",level=\"{level}\"")
            }
            "publications_total" => format!("status=\"{}\"", metric.label),
            "ig_api_errors_total" => format!("code=\"{}\"", metric.label),
            "verification_rejected_total" | "dedup_blocked_total" | "factcheck_failed_total" => {
                format!("reason=\"{}\"", metric.label)
            }
            "articles_ingested_total" => format!("layer=\"{}\"", metric.label),
            _ => format!("stage=\"{}\"", metric.label),
        };
        if histogram {
            for (limit, count) in limits.into_iter().zip(&metric.buckets) {
                let _ = writeln!(
                    out,
                    "{}_bucket{{{labels},le=\"{limit}\"}} {count}",
                    metric.name
                );
            }
            let _ = writeln!(
                out,
                "{}_sum{{{labels}}} {}\n{}_count{{{labels}}} {}",
                metric.name, metric.total, metric.name, metric.samples
            );
        } else {
            let _ = writeln!(out, "{}{{{labels}}} {}", metric.name, metric.total);
        }
    }
    for (name, value) in [
        ("queue_depth", snapshot.queue_depth as f64),
        ("dead_letter_depth", snapshot.dead_jobs as f64),
        (
            "daily_budget_remaining",
            snapshot.daily_budget_remaining as f64,
        ),
        ("process_uptime_seconds", uptime()),
        ("ig_token_verified", if token_valid { 1.0 } else { 0.0 }),
    ] {
        let _ = writeln!(out, "# TYPE {name} gauge\n{name} {value}");
    }
    if let Some(days) = token_days {
        let _ = writeln!(
            out,
            "# TYPE ig_token_days_left gauge\nig_token_days_left {days}"
        );
    }
    out.push_str("# TYPE source_up gauge\n");
    for source in &snapshot.sources {
        let _ = writeln!(
            out,
            "source_up{{source=\"{}\"}} {}",
            source.id,
            u8::from(source.up)
        );
    }
    out
}
pub(crate) async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    match state
        .store
        .operational_snapshot(state.dry_run, state.daily_cap)
        .await
    {
        Ok(snapshot) => {
            let token = state.token_state.view().await;
            let Some(exporter) = recorder() else {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            let mut body = prometheus(
                &snapshot,
                token.and_then(|t| t.days_left),
                state.token_state.allowed().await,
            );
            body.push_str(&exporter.handle().render());
            if let Some(rss) = resident_bytes().await {
                let _ = writeln!(
                    body,
                    "# TYPE process_resident_memory_bytes gauge\nprocess_resident_memory_bytes {rss}"
                );
            }
            (
                [(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
                body,
            )
                .into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
pub(crate) async fn stats(State(state): State<Arc<AppState>>) -> Response {
    match state.store.operational_snapshot(state.dry_run,state.daily_cap).await {
        Ok(snapshot)=>Json(json!({"uptime_seconds":uptime(),"resident_memory_bytes":resident_bytes().await,"dry_run":state.dry_run,"snapshot":snapshot})).into_response(),
        Err(_)=>StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
async fn resident_bytes() -> Option<u64> {
    let content = tokio::time::timeout(
        Duration::from_secs(1),
        tokio::fs::read_to_string("/proc/self/status"),
    )
    .await
    .ok()?
    .ok()?;
    content
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<u64>().ok())
        })
        .and_then(|kb| kb.checked_mul(1024))
}

struct Notifier {
    http: reqwest::Client,
    endpoint: String,
    chat_id: Secret,
}
#[derive(Debug)]
struct DeliveryFailure {
    retry_s: u32,
}
impl Notifier {
    fn new(http: reqwest::Client, token: Secret, chat_id: Secret) -> Result<Self> {
        let valid = token.expose().split_once(':').is_some_and(|(id, key)| {
            !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_digit())
                && (16..=128).contains(&key.len())
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        });
        let chat = chat_id
            .expose()
            .strip_prefix('-')
            .unwrap_or(chat_id.expose());
        if !valid || chat.is_empty() || chat.len() > 20 || !chat.bytes().all(|b| b.is_ascii_digit())
        {
            anyhow::bail!("credenciais Telegram inválidas");
        }
        Ok(Self {
            http,
            endpoint: format!("https://api.telegram.org/bot{}/sendMessage", token.expose()),
            chat_id,
        })
    }
    async fn send(&self, code: &str, subject: i64) -> std::result::Result<(), DeliveryFailure> {
        let work = async {
            let data = json!({"chat_id":self.chat_id.expose(),"text":format!("Breaking Bytes: {}. Identificador: {subject}. Consulte o painel privado e o runbook.",description(code)),"disable_notification":false,"protect_content":true});
            let mut response = self
                .http
                .post(&self.endpoint)
                .header("content-type", "application/json")
                .body(data.to_string())
                .timeout(Duration::from_secs(8))
                .send()
                .await
                .map_err(|_| DeliveryFailure { retry_s: 60 })?;
            let status = response.status().as_u16();
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| DeliveryFailure { retry_s: 60 })?
            {
                if bytes.len().saturating_add(chunk.len()) > 16_384 {
                    return Err(DeliveryFailure { retry_s: 60 });
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| DeliveryFailure { retry_s: 60 })?;
            if status == 200
                && value.get("ok").and_then(Value::as_bool) == Some(true)
                && value
                    .pointer("/result/message_id")
                    .and_then(Value::as_i64)
                    .is_some_and(|id| id > 0)
            {
                return Ok(());
            }
            let retry_s = if status == 429 {
                value
                    .pointer("/parameters/retry_after")
                    .and_then(Value::as_u64)
                    .unwrap_or(60)
                    .clamp(1, 86400) as u32
            } else if matches!(status, 401 | 403) {
                3600
            } else {
                60
            };
            Err(DeliveryFailure { retry_s })
        };
        tokio::time::timeout(Duration::from_secs(10), work)
            .await
            .unwrap_or(Err(DeliveryFailure { retry_s: 60 }))
    }
}
fn description(code: &str) -> &'static str {
    match code {
        "source_down" => "fonte sem resposta há mais de dez minutos",
        "no_news" => "nenhuma notícia nova há mais de duas horas",
        "latency_high" => "p95 de detecção até publicação acima de cinco minutos",
        "publication_failures" => "três publicações consecutivas com falha ou incerteza",
        "token_expiring" => "credencial próxima do vencimento",
        "token_invalid" => "credencial de publicação indisponível",
        "budget_exhausted" => "orçamento de publicação esgotado",
        "unsupported_text" => "checagem bloqueou texto não sustentado",
        "dead_letter" => "existem jobs na fila morta",
        "backup_stale" => "backup verificado atrasado",
        "restore_stale" => "teste de restauração mensal atrasado",
        _ => "condição operacional requer revisão",
    }
}
async fn tick(
    settings: &Settings,
    store: &Store,
    token: &TokenState,
    notifier: Option<&Notifier>,
) -> Result<()> {
    let snapshot = store
        .operational_snapshot(settings.instagram.dry_run, settings.pipeline.daily_cap)
        .await?;
    let mut conditions = Vec::with_capacity(48);
    let mut add = |code, subject| conditions.push(AlertCondition { code, subject });
    if settings.collection.enabled && !snapshot.paused {
        for source in &snapshot.sources {
            if source.silence_seconds > 600.0 {
                add("source_down", i64::from(source.id));
            }
        }
        if snapshot.no_news {
            add("no_news", 0);
        }
    }
    if snapshot.latency_high {
        add("latency_high", 0);
    }
    if snapshot.publication_failures {
        add("publication_failures", 0);
    }
    if snapshot.dead_jobs > 0 {
        add("dead_letter", 0);
    }
    if settings.publishing.enabled {
        if (settings.instagram.dry_run || token.allowed().await)
            && snapshot.daily_budget_remaining == 0
        {
            add("budget_exhausted", 0);
        }
        if !settings.instagram.dry_run {
            let view = token.view().await;
            if !token.allowed().await && (view.is_some() || uptime() > 60.0) {
                add("token_invalid", 0);
            }
            if view
                .and_then(|t| t.days_left)
                .is_some_and(|days| days < i64::from(settings.instagram.token.warning_days))
            {
                add("token_expiring", 0);
            }
        }
    }
    if settings.observability.backups_expected {
        if snapshot.backup_stale {
            add("backup_stale", 0);
        }
        if snapshot.restore_stale {
            add("restore_stale", 0);
        }
    }
    store.sync_alerts(&conditions).await?;
    if let Some(notifier) = notifier {
        for _ in 0..8 {
            let Some(lease) = store.claim_alert().await? else {
                break;
            };
            let result = notifier.send(&lease.code, lease.subject).await;
            let retry = match &result {
                Ok(()) => 3600,
                Err(e) => e
                    .retry_s
                    .max((1u32 << lease.failures.clamp(0, 10)).min(3600)),
            };
            store.finish_alert(&lease, result.is_ok(), retry).await?;
            if result.is_err() {
                tracing::warn!(
                    code = lease.code,
                    subject = lease.subject,
                    "entrega do alerta adiada"
                );
            }
        }
    }
    Ok(())
}
pub async fn run(
    settings: Settings,
    store: Store,
    http: reqwest::Client,
    token: Arc<TokenState>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    initialize()?;
    let notifier = if settings.observability.alerts_enabled {
        Some(Notifier::new(
            http,
            Secret::from_environment(&settings.observability.bot_token)?,
            Secret::from_environment(&settings.observability.chat_id)?,
        )?)
    } else {
        None
    };
    let mut interval = tokio::time::interval(Duration::from_secs(u64::from(
        settings.observability.interval_s,
    )));
    let mut upkeep = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    upkeep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while !*stop.borrow() {
        tokio::select! {
            _=crate::collection::wait_for_stop(&mut stop)=>return Ok(()),
            _=upkeep.tick()=>{if let Some(recorder)=recorder(){recorder.handle().run_upkeep();}},
            _=interval.tick()=>{
                let work=tick(&settings,&store,&token,notifier.as_ref());
                tokio::pin!(work);
                loop {
                    tokio::select! {
                        _=crate::collection::wait_for_stop(&mut stop)=>return Ok(()),
                        _=upkeep.tick()=>{if let Some(recorder)=recorder(){recorder.handle().run_upkeep();}},
                        result=&mut work=>{
                            if result.is_err(){tracing::warn!("ciclo operacional adiado; dados sensíveis omitidos");}
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
