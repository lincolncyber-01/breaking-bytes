//! Avaliação reproduzível de candidatos rotulados, sem banco ou publicação.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
use stories_config::Settings;
use stories_editorial::{
    Draft, Editorial, Task,
    client::{AnthropicClient, CompletionClient},
};
use stories_verify::Assessment;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    assessment: Assessment,
    candidate: Draft,
    expected_accepted: bool,
}

#[derive(Serialize)]
struct ResultRow {
    id: String,
    expected_accepted: bool,
    accepted: Option<bool>,
    code: String,
}

#[derive(Serialize)]
struct Report {
    model: String,
    api_version: &'static str,
    corpus: &'static str,
    total: usize,
    correct: usize,
    false_accepts: usize,
    errors: usize,
    passed: bool,
    results: Vec<ResultRow>,
}

pub async fn run(settings: &Settings, input: &Path, output: &Path) -> Result<()> {
    let metadata = tokio::fs::metadata(input)
        .await
        .context("corpus editorial indisponível")?;
    if metadata.len() > 1_048_576 {
        anyhow::bail!("corpus excede 1 MiB");
    }
    let bytes = tokio::fs::read(input).await?;
    if bytes.len() > 1_048_576 {
        anyhow::bail!("corpus excede 1 MiB");
    }
    let cases: Vec<Case> =
        tokio::task::spawn_blocking(move || serde_json::from_slice(&bytes)).await??;
    if cases.is_empty()
        || cases.len() > 200
        || cases.iter().any(|c| {
            c.id.is_empty()
                || c.id.len() > 80
                || c.assessment.sources.is_empty()
                || c.assessment.sources.len() > 8
        })
    {
        anyhow::bail!("corpus fora dos limites");
    }
    let key_name = settings
        .llm
        .api_key
        .strip_prefix("env:")
        .context("referência LLM inválida")?;
    let key = std::env::var(key_name).context("chave LLM ausente no ambiente")?;
    let transport = stories_collectors::http::HttpClient::shared(
        &settings.collection.user_agent,
        settings.collection.max_response_bytes,
        settings.collection.per_host,
    )?;
    let client = AnthropicClient::new(
        transport.trusted_api_client(),
        &key,
        &settings.llm.model,
        Duration::from_secs(settings.llm.timeout_s),
    )?;
    let engine = Arc::new(tokio::task::spawn_blocking(Editorial::new).await??);
    let mut results = Vec::new();
    for case in cases {
        results.push(evaluate(&client, engine.clone(), case).await?);
    }
    let correct = results
        .iter()
        .filter(|r| r.accepted == Some(r.expected_accepted))
        .count();
    let false_accepts = results
        .iter()
        .filter(|r| r.accepted == Some(true) && !r.expected_accepted)
        .count();
    let errors = results.iter().filter(|r| r.accepted.is_none()).count();
    let total = results.len();
    let passed = correct == total && false_accepts == 0 && errors == 0;
    let report = Report {
        model: settings.llm.model.clone(),
        api_version: "2026-10-21",
        corpus: "rotulado pelo operador; aprovação requer revisão humana",
        total,
        correct,
        false_accepts,
        errors,
        passed,
        results,
    };
    tokio::fs::write(output, serde_json::to_vec_pretty(&report)?).await?;
    if !passed {
        anyhow::bail!("avaliação editorial reprovada; consulte o relatório sem conteúdo sensível");
    }
    Ok(())
}

async fn evaluate<C: CompletionClient>(
    client: &C,
    engine: Arc<Editorial>,
    case: Case,
) -> Result<ResultRow> {
    let validator = engine.clone();
    let candidate = case.candidate.clone();
    let assessment = case.assessment.clone();
    let validation = tokio::task::spawn_blocking(move || {
        let draft = validator.parse_draft(
            serde_json::to_value(&candidate)
                .map_err(|_| stories_editorial::EditorialError::Schema)?,
            &assessment,
            &candidate.kind,
        )?;
        validator.deterministic(&draft, &assessment)
    })
    .await?;
    if let Err(error) = validation {
        return Ok(ResultRow {
            id: case.id,
            expected_accepted: case.expected_accepted,
            accepted: Some(false),
            code: error.code().into(),
        });
    }
    let data = json!({"evidence":case.assessment,"segments":case.candidate.segments(),"kind":case.candidate.kind});
    let (accepted, code) = match client.complete(Task::Check, data).await {
        Ok(value) => match tokio::task::spawn_blocking(move || {
            engine.parse_report(value, &case.candidate, &case.assessment)
        })
        .await?
        {
            Ok(_) => (Some(true), "supported"),
            Err(error) => (Some(false), error.code()),
        },
        Err(error) => (None, error.code()),
    };
    Ok(ResultRow {
        id: case.id,
        expected_accepted: case.expected_accepted,
        accepted,
        code: code.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Unavailable;
    impl CompletionClient for Unavailable {
        async fn complete(
            &self,
            _: Task,
            _: serde_json::Value,
        ) -> Result<serde_json::Value, stories_editorial::client::LlmError> {
            Err(stories_editorial::client::LlmError::Network)
        }
    }
    #[tokio::test]
    async fn evaluation_distinguishes_provider_failure_from_factual_rejection() -> Result<()> {
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../editorial/tests/fixtures/evaluation.json"
        ))?;
        let engine = Arc::new(Editorial::new()?);
        let mut local_rejections = 0;
        let mut unavailable = 0;
        for case in cases {
            let result = evaluate(&Unavailable, engine.clone(), case).await?;
            if result.accepted == Some(false) {
                local_rejections += 1;
            } else {
                assert!(result.accepted.is_none());
                unavailable += 1;
            }
        }
        assert!(local_rejections >= 3);
        assert!(unavailable >= 2);
        Ok(())
    }
}
