//! Evidência revisada conserva datas originais; elegibilidade usa reconsulta recente e origem.

use crate::{Assessment, Evidence, Level, domain, source_matches};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use stories_normalize::revision_text;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRevision {
    pub before: Evidence,
    pub after: Evidence,
    pub notice: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub after_hash: String,
}
#[derive(Debug, Error)]
pub enum RevisionError {
    #[error("snapshot de revisão inválido")]
    Invalid,
}
pub fn fingerprint(e: &Evidence, notice: Option<&str>) -> Result<[u8; 32], RevisionError> {
    let data = json!({"article":e.article_id,"source":e.source_id,"url":e.article_url,"source_url":e.source_url,"name":e.source_name,"kind":e.kind,"layer":e.layer,"weight":e.weight,"origin":e.origin_group,"date":e.published_at,"title":revision_text(&e.title),"excerpt":revision_text(&e.excerpt),"notice":notice.map(revision_text)});
    let bytes = serde_json::to_vec(&data).map_err(|_| RevisionError::Invalid)?;
    Ok(*blake3::hash(&bytes).as_bytes())
}
impl SourceRevision {
    pub fn changed(&self) -> bool {
        revision_text(&self.before.title) != revision_text(&self.after.title)
            || revision_text(&self.before.excerpt) != revision_text(&self.after.excerpt)
            || self.before.published_at != self.after.published_at
            || self
                .notice
                .as_ref()
                .is_some_and(|n| !self.before.material().contains(n))
    }
    pub fn eligible(&self, now: DateTime<Utc>, max_age_hours: u32) -> bool {
        let a = &self.after;
        let b = &self.before;
        (1..=168).contains(&max_age_hours)
            && self.observed_at <= now
            && self.observed_at >= now - Duration::hours(i64::from(max_age_hours))
            && a.article_id == b.article_id
            && a.source_id == b.source_id
            && a.article_url == b.article_url
            && a.source_url == b.source_url
            && a.source_name == b.source_name
            && a.kind == b.kind
            && a.layer == b.layer
            && a.weight == b.weight
            && a.origin_group == b.origin_group
            && a.layer != "C"
            && a.kind != "hackernews"
            && source_matches(a)
            && !a.title.trim().is_empty()
            && a.title.chars().count() <= 512
            && !a.excerpt.trim().is_empty()
            && a.excerpt.chars().count() <= 2000
            && a.published_at.is_none_or(|date| date <= now)
            && self.notice.as_ref().is_none_or(|n| {
                !n.trim().is_empty() && n.chars().count() <= 500 && a.material().contains(n)
            })
            && fingerprint(a, self.notice.as_deref())
                .is_ok_and(|h| blake3::Hash::from_bytes(h).to_hex().as_str() == self.after_hash)
    }
}

pub fn assess(changes: &[SourceRevision], now: DateTime<Utc>, max_age_hours: u32) -> Assessment {
    let empty = || Assessment {
        level: Level::Observation,
        reason: "revision_unconfirmed".into(),
        sources: Vec::new(),
    };
    if changes.is_empty() || changes.len() > 4 {
        return empty();
    }
    let mut candidates = changes
        .iter()
        .filter(|c| c.changed() && c.eligible(now, max_age_hours))
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.after
            .weight
            .cmp(&a.after.weight)
            .then_with(|| a.after.article_id.cmp(&b.after.article_id))
            .then_with(|| a.after.source_id.cmp(&b.after.source_id))
    });
    if let Some(c) = candidates
        .iter()
        .find(|c| c.after.layer == "A" || c.after.weight == 5)
    {
        return Assessment {
            level: Level::Primary,
            reason: "source_revision_primary".into(),
            sources: vec![c.after.clone()],
        };
    }
    let mut domains = BTreeSet::new();
    let mut origins = BTreeSet::new();
    let mut texts = BTreeSet::new();
    let mut articles = BTreeSet::new();
    let mut independent = Vec::new();
    for c in candidates {
        let Some(host) = domain(&c.after.article_url) else {
            continue;
        };
        let Some(origin) = c
            .after
            .origin_group
            .as_ref()
            .filter(|v| !v.trim().is_empty())
            .map(|v| stories_normalize::normalized_text(v))
        else {
            continue;
        };
        let text = revision_text(&c.after.excerpt);
        if domains.contains(&host)
            || origins.contains(&origin)
            || texts.contains(&text)
            || articles.contains(&c.after.article_id)
        {
            continue;
        }
        domains.insert(host);
        origins.insert(origin);
        texts.insert(text);
        articles.insert(c.after.article_id);
        independent.push(c.after.clone());
        if independent.len() == 2 {
            return Assessment {
                level: Level::Independent,
                reason: "source_revision_independent".into(),
                sources: independent,
            };
        }
    }
    empty()
}
