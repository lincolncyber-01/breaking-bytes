//! Transporte limitado a um endpoint fixo; erros nunca incluem corpo, chave ou URL.

use crate::{CHECK_SYSTEM, DRAFT_SYSTEM, Editorial, Task};
use reqwest::{
    Client,
    header::{CONTENT_TYPE, HeaderValue},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, time::Duration};
use thiserror::Error;

pub trait CompletionClient: Send + Sync {
    fn complete(
        &self,
        task: Task,
        data: Value,
    ) -> impl Future<Output = Result<Value, LlmError>> + Send;
}

#[derive(Clone)]
pub struct AnthropicClient {
    http: Client,
    key: HeaderValue,
    model: String,
    timeout: Duration,
    endpoint: String,
}

impl AnthropicClient {
    pub fn new(http: Client, key: &str, model: &str, timeout: Duration) -> Result<Self, LlmError> {
        if key.is_empty()
            || key.len() > 4096
            || !model.starts_with("claude-")
            || model.len() > 100
            || model.chars().any(char::is_control)
            || timeout.is_zero()
            || timeout > Duration::from_secs(60)
        {
            return Err(LlmError::Configuration);
        }
        let mut key = HeaderValue::from_str(key).map_err(|_| LlmError::Configuration)?;
        key.set_sensitive(true);
        Ok(Self {
            http,
            key,
            model: model.into(),
            timeout,
            endpoint: "https://api.anthropic.com/v1/messages".into(),
        })
    }

    #[cfg(test)]
    fn local(http: Client, key: &str, endpoint: &str) -> Result<Self, LlmError> {
        let mut client = Self::new(http, key, "claude-haiku-5-5", Duration::from_secs(1))?;
        client.endpoint = endpoint.into();
        Ok(client)
    }
}

impl CompletionClient for AnthropicClient {
    async fn complete(&self, task: Task, data: Value) -> Result<Value, LlmError> {
        let content = serde_json::to_string(&data).map_err(|_| LlmError::Input)?;
        if content.len() > 65_536 {
            return Err(LlmError::Input);
        }
        let body = json!({"model":self.model,"max_tokens":4096,"thinking":{"type":"disabled"},
            "system":match task{Task::Draft=>DRAFT_SYSTEM,Task::Check=>CHECK_SYSTEM},
            "messages":[{"role":"user","content":content}],
            "output_config":{"effort":"medium","format":{"type":"json_schema","schema":Editorial::schema(task)}}});
        tokio::time::timeout(self.timeout, async {
            let mut response = self
                .http
                .post(&self.endpoint)
                .header("x-api-key", self.key.clone())
                .header("anthropic-version", "2026-10-21")
                .header(CONTENT_TYPE, "application/json")
                .timeout(self.timeout)
                .body(body.to_string())
                .send()
                .await
                .map_err(|_| LlmError::Network)?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(|v| v.clamp(1, 86_400));
            if status != 200 {
                return Err(LlmError::Status {
                    status,
                    retry_after,
                });
            }
            if response.content_length().is_some_and(|n| n > 131_072) {
                return Err(LlmError::Limit);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| LlmError::Network)? {
                if bytes.len().saturating_add(chunk.len()) > 131_072 {
                    return Err(LlmError::Limit);
                }
                bytes.extend_from_slice(&chunk);
            }
            tokio::task::spawn_blocking(move || decode(&bytes))
                .await
                .map_err(|_| LlmError::Response)?
        })
        .await
        .map_err(|_| LlmError::Timeout)?
    }
}

#[derive(Deserialize)]
struct Message {
    stop_reason: String,
    content: Vec<Block>,
}
#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

fn decode(bytes: &[u8]) -> Result<Value, LlmError> {
    let message: Message = serde_json::from_slice(bytes).map_err(|_| LlmError::Response)?;
    if message.stop_reason == "refusal" {
        return Err(LlmError::Refusal);
    }
    if message.stop_reason != "end_turn" || message.content.len() > 8 {
        return Err(LlmError::Response);
    }
    let mut text = None;
    for block in message.content {
        match block.kind.as_str() {
            "text" if text.is_none() => text = block.text,
            "thinking" | "redacted_thinking" => {}
            _ => return Err(LlmError::Response),
        }
    }
    let text = text.ok_or(LlmError::Response)?;
    if text.len() > 65_536 {
        return Err(LlmError::Limit);
    }
    serde_json::from_str(&text).map_err(|_| LlmError::Response)
}

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("configuração LLM inválida")]
    Configuration,
    #[error("entrada LLM excede os limites")]
    Input,
    #[error("falha de rede do provedor LLM")]
    Network,
    #[error("chamada LLM excedeu o timeout")]
    Timeout,
    #[error("resposta LLM excedeu o limite de bytes")]
    Limit,
    #[error("resposta LLM inválida, truncada ou contém ação não permitida")]
    Response,
    #[error("modelo recusou a solicitação")]
    Refusal,
    #[error("provedor LLM retornou HTTP {status}")]
    Status {
        status: u16,
        retry_after: Option<u64>,
    },
}

impl LlmError {
    pub fn transient(&self) -> bool {
        matches!(
            self,
            Self::Network
                | Self::Timeout
                | Self::Status {
                    status: 429 | 500..=599,
                    ..
                }
        )
    }
    pub fn retry_after(&self) -> Option<u64> {
        if let Self::Status { retry_after, .. } = self {
            *retry_after
        } else {
            None
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Configuration => "llm_configuration",
            Self::Input => "llm_input",
            Self::Network => "llm_network",
            Self::Timeout => "llm_timeout",
            Self::Limit => "llm_limit",
            Self::Response => "llm_response",
            Self::Refusal => "llm_refusal",
            Self::Status {
                status: 401 | 403, ..
            } => "llm_auth",
            Self::Status { .. } => "llm_status",
        }
    }
}

#[cfg(test)]
mod tests;
