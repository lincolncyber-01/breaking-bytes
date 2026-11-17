//! Fronteiras externas de publicação: hosts fixos e mensagens de erro sem credenciais.

pub mod graph;
pub mod r2;
pub mod token;

use std::time::Duration;
use thiserror::Error;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_JSON: usize = 256 * 1024;
pub const MAX_JPEG: usize = 8 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("configuração de publicação inválida")]
    Configuration,
    #[error("credencial de publicação inválida ou revogada")]
    Authentication,
    #[error("permissão de publicação ausente ou revogada")]
    Permission,
    #[error("requisição externa excedeu o tempo limite ou falhou no transporte")]
    Transport,
    #[error("serviço externo retornou HTTP {0}")]
    Http(u16),
    #[error("serviço externo retornou HTTP {status}, retry-after {retry_after_s} segundos")]
    HttpWithRetry {status:u16,retry_after_s:u64},
    #[error("Graph retornou HTTP {status}, código {code}")]
    Graph {status:u16,code:u32,retry_after_s:Option<u64>},
    #[error("resposta externa inválida ou acima do limite")]
    Response,
    #[error("não foi possível assinar a operação R2")]
    Signing,
    #[error("JPEG difere do hash ou excede o limite")]
    Artifact,
}

async fn body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, PublishError> {
    if response.content_length().is_some_and(|v| v > limit as u64) {
        return Err(PublishError::Response);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| PublishError::Transport)?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(PublishError::Response);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn numeric_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|v| v.is_ascii_digit())
}

async fn digest(bytes: Vec<u8>) -> Result<(Vec<u8>, [u8; 32]), PublishError> {
    tokio::task::spawn_blocking(move || {
        let hash = *blake3::hash(&bytes).as_bytes();
        (bytes, hash)
    })
    .await
    .map_err(|_| PublishError::Artifact)
}

fn retry_after(response:&reqwest::Response)->Option<u64> {
    let header=response.headers().get("retry-after")?.to_str().ok()?;
    header.parse::<u64>().ok().or_else(||httpdate::parse_http_date(header).ok().map(|t|t.duration_since(std::time::SystemTime::now()).map(|d|d.as_secs()).unwrap_or(1))).map(|s|s.clamp(1,86400))
}
fn response_error(response:&reqwest::Response)->PublishError {
    match retry_after(response){Some(seconds)=>PublishError::HttpWithRetry{status:response.status().as_u16(),retry_after_s:seconds},None=>PublishError::Http(response.status().as_u16())}
}
impl PublishError {
    pub fn retry_after(&self)->Option<u64>{match self{Self::HttpWithRetry{retry_after_s,..}=>Some(*retry_after_s),Self::Graph{retry_after_s,..}=>*retry_after_s,_=>None}}
    pub fn code(&self)->u32{match self{Self::Graph{code,..}=>*code,Self::Http(s)|Self::HttpWithRetry{status:s,..}=>u32::from(*s),Self::Authentication=>190,Self::Permission=>200,_=>0}}

    pub fn token_failure(&self) -> Option<stories_core::token::TokenFailure> {
        match self {
            Self::Authentication => Some(stories_core::token::TokenFailure::Invalid),
            Self::Permission => Some(stories_core::token::TokenFailure::Permission),
            _ => None,
        }
    }
}
