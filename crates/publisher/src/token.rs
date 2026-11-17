//! Contratos OAuth documentados. Tokens nunca implementam Serialize nem Debug aberto.

use crate::graph::Login;
use crate::{PublishError, TIMEOUT, body, numeric_id};
use chrono::{DateTime, Duration, Utc};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderValue},
};
use serde::Deserialize;
use serde_json::Value;
use std::fmt;
use stories_core::token::{TokenFailure, TokenKind, TokenObservation, TokenSource};
use subtle::ConstantTimeEq;
use thiserror::Error;
use url::Url;

#[derive(Clone)]
pub struct Credential(String);
impl Credential {
    pub fn new(value: &str) -> Result<Self, TokenError> {
        if value.is_empty()
            || value.len() > 8192
            || value
                .bytes()
                .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        {
            return Err(TokenError::Configuration);
        }
        Ok(Self(value.into()))
    }
    pub fn fingerprint(&self) -> [u8; 32] {
        *blake3::hash(self.0.as_bytes()).as_bytes()
    }
    fn authorization(&self) -> Result<HeaderValue, TokenError> {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", self.0))
            .map_err(|_| TokenError::Configuration)?;
        value.set_sensitive(true);
        Ok(value)
    }
}
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[CREDENCIAL OCULTA]")
    }
}
#[derive(Debug, Error)]
pub enum TokenError {
    #[error("configuração ou metadados OAuth inválidos")]
    Configuration,
    #[error("token inválido ou expirado")]
    Invalid,
    #[error("permissões de publicação ausentes")]
    Permission,
    #[error("token pertence a outro aplicativo")]
    WrongApp,
    #[error("token pertence a outra Página")]
    WrongAccount,
    #[error("resposta OAuth inválida")]
    Response,
    #[error("operação não documentada para este fluxo de login")]
    Unsupported,
    #[error(transparent)]
    Api(#[from] PublishError),
}
impl TokenError {
    pub fn failure(&self) -> TokenFailure {
        match self {
            Self::Invalid => TokenFailure::Invalid,
            Self::Permission => TokenFailure::Permission,
            Self::WrongApp => TokenFailure::WrongApp,
            Self::WrongAccount => TokenFailure::WrongAccount,
            Self::Configuration => TokenFailure::Receipt,
            _ => TokenFailure::InspectionFailed,
        }
    }
}

#[derive(Clone)]
pub struct TokenClient {
    http: Client,
    base: Url,
    login: Login,
}
#[derive(Debug)]
pub enum RefreshOutcome {
    Unchanged { expires_at: DateTime<Utc> },
    RotationRequired,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuthReceipt {
    fingerprint: String,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    scopes: Vec<String>,
}

/// O recibo vem da emissão OAuth/configuração do operador; não simula introspecção remota.
pub fn instagram_receipt(
    receipt: &str,
    credential: &Credential,
    now: DateTime<Utc>,
) -> Result<TokenObservation, TokenError> {
    if receipt.len() > 16384 {
        return Err(TokenError::Configuration);
    }
    let receipt: OAuthReceipt =
        serde_json::from_str(receipt).map_err(|_| TokenError::Configuration)?;
    let expected = blake3::Hash::from_bytes(credential.fingerprint())
        .to_hex()
        .to_string();
    if !bool::from(receipt.fingerprint.as_bytes().ct_eq(expected.as_bytes()))
        || receipt.expires_at - receipt.issued_at <= Duration::days(1)
        || receipt.expires_at - receipt.issued_at > Duration::days(60)
        || receipt.issued_at > now + Duration::seconds(5)
    {
        return Err(TokenError::Configuration);
    }
    let scopes = metadata_scopes(receipt.scopes)?;
    let observation = TokenObservation {
        kind: TokenKind::Instagram,
        source: TokenSource::OAuthEnvironment,
        valid: true,
        app_id: None,
        profile_id: None,
        issued_at: Some(receipt.issued_at),
        expires_at: Some(receipt.expires_at),
        data_access_expires_at: None,
        checked_at: now,
        refreshed_at: None,
        scopes,
        rotation_required: false,
        failure: None,
    };
    if !observation.shape_valid() {
        return Err(TokenError::Configuration);
    }
    // Recibo antigo pode corresponder a token cujo prazo foi estendido e persistido no banco.
    let mut check = observation.clone();
    check.expires_at = Some(now + Duration::seconds(1));
    check.issued_at = None;
    if !check.publication_allowed(now, 300) {
        return Err(TokenError::Permission);
    }
    Ok(observation)
}

fn metadata_scopes(mut scopes: Vec<String>) -> Result<Vec<String>, TokenError> {
    if scopes.len() > 128 {
        return Err(TokenError::Response);
    }
    scopes.retain(|s| stories_core::token::metadata_scope(s));
    scopes.sort();
    scopes.dedup();
    Ok(scopes)
}

impl TokenClient {
    pub fn new(http: Client, login: Login, version: &str) -> Result<Self, TokenError> {
        let valid = version
            .strip_prefix('v')
            .and_then(|v| v.split_once('.'))
            .is_some_and(|(a, b)| {
                !a.is_empty()
                    && !b.is_empty()
                    && a.bytes().chain(b.bytes()).all(|v| v.is_ascii_digit())
            });
        if !valid {
            return Err(TokenError::Configuration);
        }
        let base = match login {
            Login::Facebook => format!("https://graph.facebook.com/{version}/"),
            Login::Instagram => "https://graph.instagram.com/".into(),
        };
        Ok(Self {
            http,
            base: Url::parse(&base).map_err(|_| TokenError::Configuration)?,
            login,
        })
    }
    #[cfg(feature = "test-support")]
    pub fn test_server(http: Client, login: Login, endpoint: &str) -> Result<Self, TokenError> {
        let base = Url::parse(endpoint).map_err(|_| TokenError::Configuration)?;
        if base.scheme() != "http"
            || base
                .host_str()
                .and_then(|v| v.parse::<std::net::IpAddr>().ok())
                .is_none_or(|v| !v.is_loopback())
        {
            return Err(TokenError::Configuration);
        }
        Ok(Self { http, base, login })
    }
    async fn request(
        &self,
        path: &str,
        query: &[(&str, &str)],
        authorization: Option<&Credential>,
    ) -> Result<Value, TokenError> {
        let mut url = self
            .base
            .join(path)
            .map_err(|_| TokenError::Configuration)?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let mut request = self.http.get(url).timeout(TIMEOUT);
        if let Some(token) = authorization {
            request = request.header(AUTHORIZATION, token.authorization()?);
        }
        tokio::time::timeout(TIMEOUT, async {
            let response = request.send().await.map_err(|_| PublishError::Transport)?;
            if !response.status().is_success() {
                return Err(PublishError::Http(response.status().as_u16()).into());
            }
            serde_json::from_slice(&body(response, 64 * 1024).await?)
                .map_err(|_| TokenError::Response)
        })
        .await
        .map_err(|_| TokenError::Api(PublishError::Transport))?
    }
    pub async fn inspect_facebook(
        &self,
        credential: &Credential,
        inspector: &Credential,
        app_id: &str,
        page_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<TokenObservation, TokenError> {
        if !matches!(self.login, Login::Facebook) {
            return Err(TokenError::Unsupported);
        }
        if !numeric_id(app_id) || page_id.is_some_and(|id| !numeric_id(id)) {
            return Err(TokenError::Configuration);
        }
        let value = self
            .request(
                "debug_token",
                &[("input_token", &credential.0)],
                Some(inspector),
            )
            .await?;
        let data = value.get("data").ok_or(TokenError::Response)?;
        if data.get("is_valid").and_then(Value::as_bool) != Some(true)
            || data.get("error").is_some_and(|v| !v.is_null())
        {
            return Err(TokenError::Invalid);
        }
        #[derive(Deserialize)]
        struct Data {
            app_id: String,
            #[serde(rename = "type")]
            kind: String,
            expires_at: i64,
            #[serde(default)]
            issued_at: i64,
            #[serde(default)]
            data_access_expires_at: i64,
            #[serde(default)]
            profile_id: Option<String>,
            scopes: Vec<String>,
        }
        let data: Data = serde_json::from_value(data.clone()).map_err(|_| TokenError::Response)?;
        if data.app_id != app_id {
            return Err(TokenError::WrongApp);
        }
        let kind = match data.kind.as_str() {
            "USER" => TokenKind::FacebookUser,
            "PAGE" => TokenKind::FacebookPage,
            _ => return Err(TokenError::Unsupported),
        };
        if kind == TokenKind::FacebookPage
            && (page_id.is_none() || data.profile_id.as_deref() != page_id)
        {
            return Err(TokenError::WrongAccount);
        }
        let date = |v: i64| -> Result<Option<DateTime<Utc>>, TokenError> {
            if v == 0 {
                Ok(None)
            } else if v < 0 {
                Err(TokenError::Response)
            } else {
                DateTime::from_timestamp(v, 0)
                    .map(Some)
                    .ok_or(TokenError::Response)
            }
        };
        let scopes = metadata_scopes(data.scopes)?;
        let observation = TokenObservation {
            kind,
            source: TokenSource::FacebookDebug,
            valid: true,
            app_id: Some(data.app_id),
            profile_id: data.profile_id,
            issued_at: date(data.issued_at)?,
            expires_at: date(data.expires_at)?,
            data_access_expires_at: date(data.data_access_expires_at)?,
            checked_at: now,
            refreshed_at: None,
            scopes,
            rotation_required: false,
            failure: None,
        };
        if !observation.shape_valid() {
            return Err(TokenError::Response);
        }
        let mut check = observation.clone();
        check.expires_at = Some(now + Duration::seconds(1));
        check.data_access_expires_at = None;
        check.issued_at = None;
        if !check.publication_allowed(now, 300) {
            return Err(TokenError::Permission);
        }
        if !observation.publication_allowed(now, 300) {
            return Err(TokenError::Invalid);
        }
        Ok(observation)
    }
    /// A resposta só é aceita automaticamente se preservar a credencial fornecida por ambiente.
    pub async fn refresh_instagram(
        &self,
        credential: &Credential,
        now: DateTime<Utc>,
    ) -> Result<RefreshOutcome, TokenError> {
        if !matches!(self.login, Login::Instagram) {
            return Err(TokenError::Unsupported);
        }
        let value = self
            .request(
                "refresh_access_token",
                &[
                    ("grant_type", "ig_refresh_token"),
                    ("access_token", &credential.0),
                ],
                None,
            )
            .await?;
        #[derive(Deserialize)]
        struct Response {
            access_token: String,
            token_type: String,
            expires_in: i64,
        }
        let response: Response = serde_json::from_value(value).map_err(|_| TokenError::Response)?;
        let candidate =
            Credential::new(&response.access_token).map_err(|_| TokenError::Response)?;
        if !response.token_type.eq_ignore_ascii_case("bearer")
            || !(86401..=60 * 86400).contains(&response.expires_in)
        {
            return Err(TokenError::Response);
        }
        if bool::from(candidate.fingerprint().ct_eq(&credential.fingerprint())) {
            Ok(RefreshOutcome::Unchanged {
                expires_at: now + Duration::seconds(response.expires_in),
            })
        } else {
            Ok(RefreshOutcome::RotationRequired)
        }
    }
}

#[cfg(test)]
mod tests;
