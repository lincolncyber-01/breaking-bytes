//! Tipos de domínio e invariantes compartilhadas pelo pipeline.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Metadados capturados: conteúdo externo é dado, sem autoridade sobre o pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedArticle {
    pub url: String,
    pub title: String,
    pub excerpt: String,
    pub image_url: Option<String>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Estados persistidos com os nomes definidos no blueprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventState {
    Detected,
    Clustered,
    Verified,
    Drafted,
    Checked,
    Rendered,
    Queued,
    Publishing,
    Published,
    Rejected,
    Superseded,
    Failed,
    NeedsCorrection,
}

impl EventState {
    pub const ALL: [Self; 13] = [
        Self::Detected,
        Self::Clustered,
        Self::Verified,
        Self::Drafted,
        Self::Checked,
        Self::Rendered,
        Self::Queued,
        Self::Publishing,
        Self::Published,
        Self::Rejected,
        Self::Superseded,
        Self::Failed,
        Self::NeedsCorrection,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Detected => "DETECTED",
            Self::Clustered => "CLUSTERED",
            Self::Verified => "VERIFIED",
            Self::Drafted => "DRAFTED",
            Self::Checked => "CHECKED",
            Self::Rendered => "RENDERED",
            Self::Queued => "QUEUED",
            Self::Publishing => "PUBLISHING",
            Self::Published => "PUBLISHED",
            Self::Rejected => "REJECTED",
            Self::Superseded => "SUPERSEDED",
            Self::Failed => "FAILED",
            Self::NeedsCorrection => "NEEDS_CORRECTION",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or(DomainError::UnknownState)
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Rejected | Self::Superseded | Self::Failed | Self::NeedsCorrection
        )
    }

    /// Não permite saltar a checagem editorial nem repetir uma transição.
    pub fn advance(self, next: Self, reason: Option<&str>) -> Result<Self, DomainError> {
        let normal = matches!(
            (self, next),
            (Self::Detected, Self::Clustered)
                | (Self::Clustered, Self::Verified)
                | (Self::Verified, Self::Drafted)
                | (Self::Drafted, Self::Checked)
                | (Self::Checked, Self::Rendered)
                | (Self::Rendered, Self::Queued)
                | (Self::Queued, Self::Publishing)
                | (Self::Publishing, Self::Published)
                | (Self::Published, Self::NeedsCorrection)
        );
        let rejection = !self.is_terminal()
            && self != Self::Published
            && matches!(next, Self::Rejected | Self::Superseded | Self::Failed);
        if !normal && !rejection {
            return Err(DomainError::InvalidTransition {
                from: self,
                to: next,
            });
        }
        if next.is_terminal() && reason.is_none_or(|value| value.trim().is_empty()) {
            return Err(DomainError::ReasonRequired);
        }
        Ok(next)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicationKind {
    New,
    Update,
    Correction,
}

impl PublicationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Update => "update",
            Self::Correction => "correction",
        }
    }
}

/// Chave imutável da intenção de publicação, compartilhada por todas as tentativas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationKey {
    event_id: i64,
    kind: PublicationKind,
    sequence: i32,
}

impl PublicationKey {
    pub fn new(event_id: i64, kind: PublicationKind, sequence: i32) -> Result<Self, DomainError> {
        if event_id <= 0 || sequence < 0 {
            return Err(DomainError::InvalidPublicationKey);
        }
        Ok(Self {
            event_id,
            kind,
            sequence,
        })
    }
    pub const fn event_id(self) -> i64 {
        self.event_id
    }
    pub const fn kind(self) -> PublicationKind {
        self.kind
    }
    pub const fn sequence(self) -> i32 {
        self.sequence
    }
}

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("estado persistido desconhecido")]
    UnknownState,
    #[error("transição não permitida: {from:?} -> {to:?}")]
    InvalidTransition { from: EventState, to: EventState },
    #[error("estado terminal exige um motivo")]
    ReasonRequired,
    #[error("chave de publicação inválida")]
    InvalidPublicationKey,
}
