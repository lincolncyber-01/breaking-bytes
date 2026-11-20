//! Estado de credenciais sem conter o token ou qualquer segredo de inspeção.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    Unknown,
    FacebookUser,
    FacebookPage,
    Instagram,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    Unverified,
    FacebookDebug,
    OAuthEnvironment,
    InstagramRefresh,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenFailure {
    InspectionFailed,
    Invalid,
    Permission,
    WrongApp,
    WrongAccount,
    Receipt,
    RefreshUncertain,
    RotationRequired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenStatus {
    Unverified,
    Valid,
    Expiring,
    Expired,
    Invalid,
    Stale,
    RotationRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenObservation {
    pub kind: TokenKind,
    pub source: TokenSource,
    pub valid: bool,
    pub app_id: Option<String>,
    pub profile_id: Option<String>,
    pub issued_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub data_access_expires_at: Option<DateTime<Utc>>,
    pub checked_at: DateTime<Utc>,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
    pub rotation_required: bool,
    pub failure: Option<TokenFailure>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenView {
    pub status: TokenStatus,
    pub kind: TokenKind,
    pub source: TokenSource,
    pub days_left: Option<i64>,
    pub checked_at: DateTime<Utc>,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub failure: Option<TokenFailure>,
}
/// Conjunto mínimo usado pelo publicador; campos arbitrários da API não são persistidos.
pub fn metadata_scope(scope: &str) -> bool {
    matches!(
        scope,
        "instagram_basic"
            | "instagram_content_publish"
            | "pages_read_engagement"
            | "ads_read"
            | "ads_management"
            | "instagram_business_basic"
            | "instagram_business_content_publish"
    )
}
impl TokenObservation {
    pub fn unverified(now: DateTime<Utc>, failure: TokenFailure) -> Self {
        Self {
            kind: TokenKind::Unknown,
            source: TokenSource::Unverified,
            valid: false,
            app_id: None,
            profile_id: None,
            issued_at: None,
            expires_at: None,
            data_access_expires_at: None,
            checked_at: now,
            refreshed_at: None,
            scopes: Vec::new(),
            rotation_required: false,
            failure: Some(failure),
        }
    }
    pub fn shape_valid(&self) -> bool {
        [&self.app_id, &self.profile_id].iter().all(|id| {
            id.as_ref().is_none_or(|s| {
                !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_digit())
            })
        }) && self.scopes.len() <= 128
            && self
                .scopes
                .iter()
                .all(|s| !s.is_empty() && s.len() <= 128 && metadata_scope(s))
            && self
                .issued_at
                .is_none_or(|t| t <= self.checked_at + Duration::seconds(5))
            && self
                .expires_at
                .zip(self.issued_at)
                .is_none_or(|(end, start)| end > start)
    }
    pub fn deadline(&self) -> Option<DateTime<Utc>> {
        match (self.expires_at, self.data_access_expires_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
    pub fn view(&self, now: DateTime<Utc>, max_stale_s: i64, warning_days: i64) -> TokenView {
        let deadline = self.deadline();
        let status = if self.rotation_required {
            TokenStatus::RotationRequired
        } else if !self.valid || self.failure.is_some() || !self.shape_valid() {
            TokenStatus::Invalid
        } else if deadline.is_some_and(|t| t <= now) {
            TokenStatus::Expired
        } else if self.checked_at > now + Duration::seconds(5)
            || now - self.checked_at > Duration::seconds(max_stale_s.clamp(1, 3600))
        {
            TokenStatus::Stale
        } else if !self.permissions_valid()
            || self.kind != TokenKind::FacebookPage && self.expires_at.is_none()
        {
            TokenStatus::Invalid
        } else if deadline.is_some_and(|t| t - now < Duration::days(warning_days.clamp(1, 14))) {
            TokenStatus::Expiring
        } else {
            TokenStatus::Valid
        };
        TokenView {
            status,
            kind: self.kind,
            source: self.source,
            days_left: deadline.map(|t| (t - now).num_days()),
            checked_at: self.checked_at,
            refreshed_at: self.refreshed_at,
            failure: self.failure,
        }
    }
    pub fn publication_allowed(&self, now: DateTime<Utc>, max_stale_s: i64) -> bool {
        matches!(
            self.view(now, max_stale_s, 7).status,
            TokenStatus::Valid | TokenStatus::Expiring
        )
    }
    pub fn refresh_due(&self, now: DateTime<Utc>, refresh_days: i64) -> bool {
        if self.kind != TokenKind::Instagram || !self.publication_allowed(now, 3600) {
            return false;
        }
        let Some(issued) = self.issued_at else {
            return false;
        };
        now - issued >= Duration::days(1)
            && now - self.refreshed_at.unwrap_or(issued)
                >= Duration::days(refresh_days.clamp(1, 30))
    }
    fn permissions_valid(&self) -> bool {
        let source_matches = match self.kind {
            TokenKind::FacebookUser | TokenKind::FacebookPage => {
                self.source == TokenSource::FacebookDebug
                    && self.app_id.is_some()
                    && (self.kind != TokenKind::FacebookPage || self.profile_id.is_some())
            }
            TokenKind::Instagram => matches!(
                self.source,
                TokenSource::OAuthEnvironment | TokenSource::InstagramRefresh
            ),
            TokenKind::Unknown => false,
        };
        if !source_matches {
            return false;
        }
        let required: &[&str] = match self.kind {
            TokenKind::FacebookUser | TokenKind::FacebookPage => &[
                "instagram_basic",
                "instagram_content_publish",
                "pages_read_engagement",
            ],
            TokenKind::Instagram => &[
                "instagram_business_basic",
                "instagram_business_content_publish",
            ],
            TokenKind::Unknown => return false,
        };
        required.iter().all(|s| self.scopes.iter().any(|v| v == s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    fn claims(now: DateTime<Utc>) -> TokenObservation {
        TokenObservation {
            kind: TokenKind::Instagram,
            source: TokenSource::OAuthEnvironment,
            valid: true,
            app_id: None,
            profile_id: None,
            issued_at: Some(now - Duration::days(30)),
            expires_at: Some(now + Duration::days(30)),
            data_access_expires_at: None,
            checked_at: now,
            refreshed_at: None,
            scopes: vec![
                "instagram_business_basic".into(),
                "instagram_business_content_publish".into(),
            ],
            rotation_required: false,
            failure: None,
        }
    }
    #[test]
    fn expiry_permissions_age_and_page_lifetime_are_distinct() {
        let now = Utc::now();
        let mut c = claims(now);
        assert!(c.refresh_due(now, 30));
        c.refreshed_at = Some(now);
        assert!(!c.refresh_due(now, 30));
        c.expires_at = Some(now);
        assert!(!c.publication_allowed(now, 300));
        c.expires_at = Some(now + Duration::days(60));
        c.data_access_expires_at = Some(now);
        assert!(!c.publication_allowed(now, 300));
        c.data_access_expires_at = None;
        c.scopes.clear();
        assert!(!c.publication_allowed(now, 300));
        c.app_id = Some("99".into());
        c.profile_id = Some("123".into());
        c.kind = TokenKind::FacebookPage;
        c.source = TokenSource::FacebookDebug;
        c.expires_at = None;
        c.scopes = vec![
            "instagram_basic".into(),
            "instagram_content_publish".into(),
            "pages_read_engagement".into(),
        ];
        assert!(c.publication_allowed(now, 300));
        assert!(!c.refresh_due(now, 30));
        c.kind = TokenKind::FacebookUser;
        assert!(!c.publication_allowed(now, 300));
    }
    proptest! {
        #[test]
        fn expired_or_stale_proofs_never_allow_publication(age in 301i64..1000000,remaining in -100000i64..=0) {
            let now=Utc::now();let mut c=claims(now);c.checked_at=now-Duration::seconds(age);
            prop_assert!(!c.publication_allowed(now,300));
            c.checked_at=now;c.expires_at=Some(now+Duration::seconds(remaining));
            prop_assert!(!c.publication_allowed(now,300));
        }
        #[test]
        fn rotation_intent_blocks_every_remaining_lifetime(days in 1i64..61) {
            let now=Utc::now();let mut c=claims(now);c.expires_at=Some(now+Duration::days(days));c.rotation_required=true;
            prop_assert!(!c.publication_allowed(now,300));prop_assert!(!c.refresh_due(now,30));
        }
    }
}
