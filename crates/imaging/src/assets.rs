//! A revisão de direitos e conteúdo é vinculada ao hash exato do arquivo.

use crate::ImagingError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use stories_normalize::normalized_text;
use url::Url;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    PressKit,
    Primary,
    Og,
    Library,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub reviewer: String,
    pub reviewed_at: DateTime<Utc>,
    pub watermark_free: bool,
    pub relevant: bool,
    pub generic: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub url: String,
    pub role: Role,
    pub article_url: Option<String>,
    pub entities: Vec<String>,
    pub credit: String,
    pub license: String,
    pub rights_url: String,
    pub blake3: String,
    pub review: Review,
}

#[derive(Default)]
pub struct Catalog {
    assets: Vec<Asset>,
    domains: Vec<String>,
}

fn https_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

impl Catalog {
    pub fn parse(bytes: &[u8], domains: Vec<String>) -> Result<Self, ImagingError> {
        if bytes.len() > 1_048_576
            || domains.len() > 32
            || domains.iter().any(|h| {
                h.is_empty() || h.contains('/') || h.contains('*') || h != &h.to_ascii_lowercase()
            })
        {
            return Err(ImagingError::Configuration);
        }
        let assets: Vec<Asset> =
            serde_json::from_slice(bytes).map_err(|_| ImagingError::Configuration)?;
        if assets.len() > 128 {
            return Err(ImagingError::Configuration);
        }
        let mut urls = std::collections::BTreeSet::new();
        for asset in &assets {
            if !urls.insert(&asset.url)
                || https_url(&asset.url).is_none()
                || https_url(&asset.rights_url).is_none()
                || !matches!(
                    asset.license.as_str(),
                    "owned" | "cc0" | "cc-by" | "press-permission"
                )
                || asset.credit.trim().is_empty()
                || asset.credit.chars().count() > 100
                || asset.credit.chars().any(char::is_control)
                || asset.blake3.len() != 64
                || !asset
                    .blake3
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                || asset.review.reviewer.trim().is_empty()
                || asset.review.reviewer.chars().count() > 100
                || asset.entities.len() > 16
                || asset
                    .entities
                    .iter()
                    .any(|e| e.is_empty() || e.chars().count() > 80)
                || asset
                    .article_url
                    .as_ref()
                    .is_some_and(|u| https_url(u).is_none())
            {
                return Err(ImagingError::Configuration);
            }
        }
        Ok(Self { assets, domains })
    }

    pub fn candidates(
        &self,
        article: &str,
        entities: &[String],
        og: Option<&str>,
        now: DateTime<Utc>,
    ) -> Vec<Asset> {
        let mut output = self
            .assets
            .iter()
            .filter(|asset| {
                let Some(url) = https_url(&asset.url) else {
                    return false;
                };
                let eligible = asset.review.watermark_free
                    && asset.review.relevant
                    && !asset.review.generic
                    && asset.review.reviewed_at <= now
                    && asset.review.reviewed_at >= now - chrono::Duration::days(180)
                    && url
                        .host_str()
                        .is_some_and(|h| self.domains.iter().any(|allowed| allowed == h));
                let same_article = asset.article_url.as_deref() == Some(article);
                let same_entities = !asset.entities.is_empty()
                    && asset.entities.iter().all(|e| {
                        entities
                            .iter()
                            .any(|known| normalized_text(known) == normalized_text(e))
                    });
                eligible
                    && (same_article || same_entities)
                    && (asset.role != Role::Og || (same_article && og == Some(asset.url.as_str())))
            })
            .cloned()
            .collect::<Vec<_>>();
        output.sort_by(|a, b| a.role.cmp(&b.role).then_with(|| a.url.cmp(&b.url)));
        output
    }
}

impl Asset {
    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<(), ImagingError> {
        if blake3::hash(bytes).to_hex().as_str() != self.blake3 {
            return Err(ImagingError::Provenance);
        }
        Ok(())
    }
}
