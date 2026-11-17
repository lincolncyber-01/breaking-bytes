//! Monitor de credencial: nenhuma escrita de segredo em banco, arquivo, log ou resposta administrativa.

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use stories_collectors::http::HttpClient;
use stories_config::{Secrets, Settings, TokenSettings};
use stories_core::token::{
    TokenFailure, TokenKind, TokenObservation, TokenSource, TokenStatus, TokenView,
};
use stories_publisher::{
    graph::Login,
    token::{Credential, RefreshOutcome, TokenClient, TokenError, instagram_receipt},
};
use stories_store::{Store, StoreError};
use tokio::sync::RwLock;

pub struct TokenState {
    observation: RwLock<Option<TokenObservation>>,
    max_stale_s: i64,
    warning_days: i64,
}
impl Default for TokenState {
    fn default() -> Self {
        Self::new(620, 7)
    }
}
impl TokenState {
    pub fn new(max_stale_s: i64, warning_days: i64) -> Self {
        Self {
            observation: RwLock::new(None),
            max_stale_s: max_stale_s.clamp(1, 3600),
            warning_days: warning_days.clamp(1, 14),
        }
    }
    pub async fn view(&self) -> Option<TokenView> {
        self.observation
            .read()
            .await
            .as_ref()
            .map(|o| o.view(Utc::now(), self.max_stale_s, self.warning_days))
    }
    pub async fn allowed(&self) -> bool {
        self.observation
            .read()
            .await
            .as_ref()
            .is_some_and(|o| o.publication_allowed(Utc::now(), self.max_stale_s))
    }
    #[cfg(test)]
    pub(crate) async fn test_install(&self, observation: TokenObservation) {
        self.update(observation).await;
    }
    async fn update(&self, observation: TokenObservation) {
        *self.observation.write().await = Some(observation);
    }
}

enum Source {
    Facebook {
        app_id: String,
        inspector: Credential,
        page_id: Option<String>,
    },
    Instagram {
        receipt: TokenObservation,
    },
}
pub(crate) struct TokenMonitor {
    client: TokenClient,
    credential: Credential,
    source: Source,
    settings: TokenSettings,
    state: Arc<TokenState>,
    first_check: AtomicBool,
}
impl TokenMonitor {
    pub(crate) fn new(
        http: &HttpClient,
        settings: &Settings,
        secrets: &Secrets,
        state: Arc<TokenState>,
    ) -> Result<Self> {
        let credential = Credential::new(
            secrets
                .instagram_token
                .as_ref()
                .context("token de publicação ausente")?
                .expose(),
        )?;
        let login = match settings.instagram.login {
            stories_config::Login::Facebook => Login::Facebook,
            stories_config::Login::Instagram => Login::Instagram,
        };
        let source = match login {
            Login::Facebook => Source::Facebook {
                app_id: secrets
                    .meta_app_id
                    .as_ref()
                    .context("META_APP_ID ausente")?
                    .expose()
                    .into(),
                inspector: Credential::new(
                    secrets
                        .meta_inspection_token
                        .as_ref()
                        .context("META_INSPECTION_TOKEN ausente")?
                        .expose(),
                )?,
                page_id: secrets.facebook_page_id.as_ref().map(|v| v.expose().into()),
            },
            Login::Instagram => Source::Instagram {
                receipt: instagram_receipt(
                    secrets
                        .instagram_token_receipt
                        .as_ref()
                        .context("IG_TOKEN_RECEIPT ausente")?
                        .expose(),
                    &credential,
                    Utc::now(),
                )?,
            },
        };
        Ok(Self {
            client: TokenClient::new(
                http.trusted_api_client(),
                login,
                &settings.instagram.api_version,
            )?,
            credential,
            source,
            settings: settings.instagram.token.clone(),
            state,
            first_check: AtomicBool::new(true),
        })
    }
    fn bound(&self, o: &TokenObservation) -> bool {
        match &self.source {
            Source::Facebook {
                app_id, page_id, ..
            } => {
                matches!(o.kind, TokenKind::FacebookUser | TokenKind::FacebookPage)
                    && o.app_id.as_deref() == Some(app_id.as_str())
                    && (o.kind != TokenKind::FacebookPage
                        || o.profile_id.as_deref() == page_id.as_deref() && page_id.is_some())
            }
            Source::Instagram { .. } => o.kind == TokenKind::Instagram,
        }
    }
    async fn publish_state(
        &self,
        store: &Store,
        account: &str,
        o: &TokenObservation,
    ) -> Result<()> {
        let mut visible = o.clone();
        if visible.valid && !self.bound(&visible) {
            visible.valid = false;
            visible.failure = Some(TokenFailure::WrongApp);
        }
        let view = visible.view(Utc::now(), self.state.max_stale_s, self.state.warning_days);
        self.state.update(visible).await;
        let code = match view.status {
            TokenStatus::Unverified => "unverified",
            TokenStatus::Valid => "valid",
            TokenStatus::Expiring => "expiring",
            TokenStatus::Expired => "expired",
            TokenStatus::Invalid => "invalid",
            TokenStatus::Stale => "stale",
            TokenStatus::RotationRequired => "rotation_required",
        };
        if view.status != TokenStatus::Valid
            && store
                .token_alert_due(account, self.credential.fingerprint(), code)
                .await?
        {
            tracing::warn!(
                code,
                days_left = view.days_left,
                "credencial de publicação exige atenção"
            );
        }
        Ok(())
    }
    pub(crate) async fn check(&self, store: &Store, account: &str) -> Result<()> {
        let now = Utc::now();
        let fingerprint = self.credential.fingerprint();
        let previous = store.token_record(account, fingerprint).await?;
        let first = self.first_check.swap(false, Ordering::Relaxed);
        let cached = previous.as_ref().is_some_and(|p| {
            self.bound(&p.observation)
                && now >= p.observation.checked_at
                && now - p.observation.checked_at
                    < Duration::seconds(i64::from(self.settings.check_interval_s))
        });
        let mut record = if cached && !first {
            previous.context("registro de token ausente")?
        } else {
            let observation = match &self.source {
                Source::Facebook {
                    app_id,
                    inspector,
                    page_id,
                } => {
                    match self
                        .client
                        .inspect_facebook(
                            &self.credential,
                            inspector,
                            app_id,
                            page_id.as_deref(),
                            now,
                        )
                        .await
                    {
                        Ok(o) => o,
                        Err(error) => {
                            let mut o = previous
                                .as_ref()
                                .map(|v| v.observation.clone())
                                .unwrap_or_else(|| {
                                    TokenObservation::unverified(now, error.failure())
                                });
                            o.valid = false;
                            o.checked_at = now;
                            o.failure = Some(error.failure());
                            o
                        }
                    }
                }
                Source::Instagram { receipt } => {
                    let mut o = previous
                        .as_ref()
                        .map(|v| v.observation.clone())
                        .unwrap_or_else(|| receipt.clone());
                    // Recibo não apaga falha, intenção incerta ou renovação confirmada deste mesmo hash.
                    o.checked_at = now;
                    o
                }
            };
            match store
                .save_token_observation(
                    account,
                    fingerprint,
                    previous.as_ref().map(|r| r.version),
                    &observation,
                )
                .await
            {
                Ok(record) => record,
                Err(StoreError::Conflict) => store
                    .token_record(account, fingerprint)
                    .await?
                    .context("metadados concorrentes ausentes")?,
                Err(error) => return Err(error.into()),
            }
        };
        if self.settings.auto_refresh
            && record
                .observation
                .refresh_due(now, i64::from(self.settings.refresh_days))
        {
            if let Some(lease) = store
                .begin_token_refresh(
                    account,
                    fingerprint,
                    record.version,
                    self.settings.refresh_days,
                )
                .await?
            {
                let mut observation = record.observation.clone();
                observation.checked_at = Utc::now();
                let mut pending = observation.clone();
                pending.rotation_required = true;
                pending.failure = Some(TokenFailure::RefreshUncertain);
                self.state.update(pending).await;
                match self.client.refresh_instagram(&self.credential, now).await {
                    Ok(RefreshOutcome::Unchanged { expires_at }) => {
                        observation.expires_at = Some(expires_at);
                        observation.refreshed_at = Some(now);
                        observation.source = TokenSource::InstagramRefresh;
                        observation.rotation_required = false;
                        observation.failure = None;
                    }
                    Ok(RefreshOutcome::RotationRequired) => {
                        observation.rotation_required = true;
                        observation.failure = Some(TokenFailure::RotationRequired);
                    }
                    Err(_) => {
                        observation.rotation_required = true;
                        observation.failure = Some(TokenFailure::RefreshUncertain);
                    }
                }
                record = store
                    .finish_token_refresh(account, fingerprint, &lease, &observation)
                    .await?;
            } else {
                record = store
                    .token_record(account, fingerprint)
                    .await?
                    .context("registro de renovação ausente")?;
            }
        }
        self.publish_state(store, account, &record.observation)
            .await?;
        if !self.bound(&record.observation)
            || !record
                .observation
                .publication_allowed(Utc::now(), self.state.max_stale_s)
        {
            return Err(TokenError::Invalid.into());
        }
        Ok(())
    }
    /// Leitura fresca do banco impede que outra réplica ignore a barreira de renovação.
    pub(crate) async fn ensure(&self, store: &Store, account: &str) -> Result<()> {
        let record = store
            .token_record(account, self.credential.fingerprint())
            .await?;
        if let Some(record) = record {
            let mut visible = record.observation.clone();
            if visible.valid && !self.bound(&visible) {
                visible.valid = false;
                visible.failure = Some(TokenFailure::WrongApp);
            }
            self.state.update(visible).await;
            if self.bound(&record.observation)
                && record
                    .observation
                    .publication_allowed(Utc::now(), self.state.max_stale_s)
            {
                return Ok(());
            }
        }
        Err(TokenError::Invalid.into())
    }
    pub(crate) async fn invalidate(
        &self,
        store: &Store,
        account: &str,
        failure: TokenFailure,
    ) -> Result<()> {
        if let Some(record) = store
            .token_record(account, self.credential.fingerprint())
            .await?
        {
            let mut o = record.observation;
            o.valid = false;
            o.checked_at = Utc::now();
            o.failure = Some(failure);
            match store
                .save_token_observation(
                    account,
                    self.credential.fingerprint(),
                    Some(record.version),
                    &o,
                )
                .await
            {
                Ok(record) => {
                    self.publish_state(store, account, &record.observation)
                        .await?
                }
                Err(StoreError::Conflict) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn test_facebook(client: TokenClient) -> Result<Self> {
        Ok(Self {
            client,
            credential: Credential::new("contract-only")?,
            source: Source::Facebook {
                app_id: "99".into(),
                inspector: Credential::new("inspector-only")?,
                page_id: None,
            },
            settings: TokenSettings::default(),
            state: Arc::new(TokenState::default()),
            first_check: AtomicBool::new(true),
        })
    }
}

#[cfg(test)]
mod tests;

/// Dependências HTTP podem imprimir URL/query em modo trace; somente o app emite diagnósticos sanitizados.
pub fn log_target_allowed(target: &str) -> bool {
    target.starts_with("stories_")
}
