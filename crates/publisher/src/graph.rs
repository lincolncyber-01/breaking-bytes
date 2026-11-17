//! Contrato da Instagram API v27: JPEG em Stories, sem caption, alt_text ou stickers.

use crate::{MAX_JSON, PublishError, TIMEOUT, body, numeric_id};
use reqwest::{
    Client, Method,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

#[derive(Clone, Copy, Debug)]
pub enum Login {
    Facebook,
    Instagram,
}

#[derive(Clone)]
pub struct Graph {
    client: Client,
    base: Url,
    user_id: String,
    authorization: HeaderValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    pub usage: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContainerStatus {
    InProgress,
    Finished,
    Published,
    Error,
    Expired,
}

impl Graph {
    pub fn new(
        client: Client,
        login: Login,
        version: &str,
        user_id: &str,
        token: &str,
    ) -> Result<Self, PublishError> {
        let host = match login {
            Login::Facebook => "graph.facebook.com",
            Login::Instagram => "graph.instagram.com",
        };
        let valid = version
            .strip_prefix('v')
            .and_then(|v| v.split_once('.'))
            .is_some_and(|(a, b)| {
                !a.is_empty()
                    && !b.is_empty()
                    && a.bytes().chain(b.bytes()).all(|v| v.is_ascii_digit())
            });
        if !valid || !numeric_id(user_id) || token.is_empty() {
            return Err(PublishError::Configuration);
        }
        let base = Url::parse(&format!("https://{host}/{version}/"))
            .map_err(|_| PublishError::Configuration)?;
        Self::build(client, base, user_id, token)
    }
    fn build(client: Client, base: Url, user_id: &str, token: &str) -> Result<Self, PublishError> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| PublishError::Configuration)?;
        authorization.set_sensitive(true);
        Ok(Self {
            client,
            base,
            user_id: user_id.into(),
            authorization,
        })
    }
    /// Disponível apenas em builds de teste; nunca aceita um host externo.
    #[cfg(feature = "test-support")]
    pub fn test_server(client: Client, endpoint: &str) -> Result<Self, PublishError> {
        let url = Url::parse(endpoint).map_err(|_| PublishError::Configuration)?;
        if url.scheme() != "http"
            || url
                .host_str()
                .and_then(|h| h.parse::<std::net::IpAddr>().ok())
                .is_none_or(|ip| !ip.is_loopback())
        {
            return Err(PublishError::Configuration);
        }
        Self::build(client, url, "123", "contract-only")
    }
    pub fn account_key(&self) -> String {
        blake3::hash(
            format!(
                "{}:{}",
                self.base.host_str().unwrap_or_default(),
                self.user_id
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string()
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        payload: Option<Value>,
    ) -> Result<Value, PublishError> {
        let mut url = self
            .base
            .join(path)
            .map_err(|_| PublishError::Configuration)?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let mut request = self
            .client
            .request(method, url)
            .header(AUTHORIZATION, self.authorization.clone())
            .timeout(TIMEOUT);
        if let Some(payload) = payload {
            request = request
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&payload).map_err(|_| PublishError::Configuration)?);
        }
        tokio::time::timeout(TIMEOUT, async {
            let response = request.send().await.map_err(|_| PublishError::Transport)?;
            let status = response.status();
            if !status.is_success() {
                return Err(PublishError::Http(status.as_u16()));
            }
            serde_json::from_slice(&body(response, MAX_JSON).await?)
                .map_err(|_| PublishError::Response)
        })
        .await
        .map_err(|_| PublishError::Transport)?
    }
    pub async fn quota(&self) -> Result<Quota, PublishError> {
        let value = self
            .request(
                Method::GET,
                &format!("{}/content_publishing_limit", self.user_id),
                &[("fields", "quota_usage,config")],
                None,
            )
            .await?;
        let data = value
            .get("data")
            .and_then(Value::as_array)
            .filter(|v| v.len() == 1)
            .and_then(|v| v.first())
            .ok_or(PublishError::Response)?;
        let read = |v: Option<&Value>| {
            v.and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(PublishError::Response)
        };
        let config = data.get("config").ok_or(PublishError::Response)?;
        if config.get("quota_duration").and_then(Value::as_u64) != Some(86400) {
            return Err(PublishError::Response);
        }
        let quota = Quota {
            usage: read(data.get("quota_usage"))?,
            total: read(config.get("quota_total"))?,
        };
        if quota.total == 0 {
            return Err(PublishError::Response);
        }
        Ok(quota)
    }
    pub async fn create(&self, image_url: &str) -> Result<String, PublishError> {
        let url = Url::parse(image_url).map_err(|_| PublishError::Configuration)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(PublishError::Configuration);
        }
        let value = self
            .request(
                Method::POST,
                &format!("{}/media", self.user_id),
                &[],
                Some(json!({"image_url":image_url,"media_type":"STORIES"})),
            )
            .await?;
        id(&value)
    }
    pub async fn status(&self, container: &str) -> Result<ContainerStatus, PublishError> {
        if !numeric_id(container) {
            return Err(PublishError::Configuration);
        }
        let value = self
            .request(Method::GET, container, &[("fields", "status_code")], None)
            .await?;
        serde_json::from_value(
            value
                .get("status_code")
                .cloned()
                .ok_or(PublishError::Response)?,
        )
        .map_err(|_| PublishError::Response)
    }
    /// Nenhum retry de escrita: qualquer erro após esta chamada exige reconciliação.
    pub async fn publish(&self, container: &str) -> Result<String, PublishError> {
        if !numeric_id(container) {
            return Err(PublishError::Configuration);
        }
        id(&self
            .request(
                Method::POST,
                &format!("{}/media_publish", self.user_id),
                &[],
                Some(json!({"creation_id":container})),
            )
            .await?)
    }
    /// Cursores são dados; URLs `paging.next` nunca são seguidas nem recebem o token.
    pub async fn stories(&self) -> Result<Vec<String>, PublishError> {
        let mut result = Vec::new();
        let mut after = None::<String>;
        for _ in 0..20 {
            let mut query = vec![("fields", "id"), ("limit", "100")];
            if let Some(ref cursor) = after {
                query.push(("after", cursor));
            }
            let value = self
                .request(
                    Method::GET,
                    &format!("{}/stories", self.user_id),
                    &query,
                    None,
                )
                .await?;
            let data = value
                .get("data")
                .and_then(Value::as_array)
                .filter(|v| v.len() <= 100)
                .ok_or(PublishError::Response)?;
            for entry in data {
                result.push(id(entry)?);
            }
            if value.pointer("/paging/next").is_none() {
                return Ok(result);
            }
            let cursor = value
                .pointer("/paging/cursors/after")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 1024)
                .ok_or(PublishError::Response)?;
            if after.as_deref() == Some(cursor) {
                return Err(PublishError::Response);
            }
            after = Some(cursor.to_owned());
        }
        Err(PublishError::Response)
    }
}
fn id(value: &Value) -> Result<String, PublishError> {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|v| numeric_id(v))
        .map(str::to_owned)
        .ok_or(PublishError::Response)
}

#[cfg(test)]
mod tests;
