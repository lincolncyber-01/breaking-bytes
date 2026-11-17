//! Elegibilidade factual conservadora, separada da geração de texto.

pub mod revisions;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use stories_normalize::normalized_text;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub article_id: i64,
    pub source_id: i32,
    pub source_name: String,
    pub source_url: String,
    pub article_url: String,
    pub layer: String,
    pub kind: String,
    pub weight: i16,
    pub origin_group: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub date_confirmed: bool,
    pub title: String,
    pub excerpt: String,
}

impl Evidence {
    pub fn material(&self) -> String {
        format!("{}\n{}", self.title, self.excerpt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    Primary = 1,
    Independent = 2,
    Observation = 3,
}

impl Level {
    pub const fn number(self) -> i16 {
        self as i16
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assessment {
    pub level: Level,
    pub reason: String,
    pub sources: Vec<Evidence>,
}

pub fn domain(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let host = parsed.host_str()?.trim_end_matches('.');
    psl::domain_str(host).map(str::to_ascii_lowercase)
}

pub fn suspicious(title: &str) -> bool {
    let text = format!(" {} ", normalized_text(title));
    [
        " rumor ",
        " rumores ",
        " allegedly ",
        " unconfirmed ",
        " suposto ",
        " suposta ",
        " nao confirmado ",
        " vazamento sem confirmacao ",
        " voce nao vai acreditar ",
        " you won t believe ",
        " ai generated ",
        " gerado por ia ",
    ]
    .iter()
    .any(|term| text.contains(term))
}

/// APIs hospedadas não conferem autoridade a outros projetos do mesmo domínio.
pub fn source_matches(e: &Evidence) -> bool {
    let (Ok(source), Ok(article)) = (Url::parse(&e.source_url), Url::parse(&e.article_url)) else {
        return false;
    };
    if e.kind == "github" {
        let parts: Vec<_> = source.path().split('/').filter(|s| !s.is_empty()).collect();
        return source.host_str() == Some("api.github.com")
            && parts.len() >= 3
            && parts[0] == "repos"
            && article.host_str() == Some("github.com")
            && article
                .path()
                .starts_with(&format!("/{}/{}/releases/", parts[1], parts[2]));
    }
    domain(&e.source_url).is_some()
        && (domain(&e.source_url) == domain(&e.article_url)
            || e.kind == "status" && article.host_str() == Some("stspg.io"))
}

pub fn assess(input: &[Evidence], now: DateTime<Utc>, max_age_hours: u32) -> Assessment {
    let observation = |reason: &str| Assessment {
        level: Level::Observation,
        reason: reason.into(),
        sources: Vec::new(),
    };
    if input.is_empty() || input.len() > 32 || !(1..=168).contains(&max_age_hours) {
        return observation("insufficient_evidence");
    }
    let mut eligible: Vec<_> = input
        .iter()
        .filter(|e| {
            e.date_confirmed
                && e.published_at.is_some_and(|date| {
                    date <= now && date >= now - chrono::Duration::hours(i64::from(max_age_hours))
                })
                && e.layer != "C"
                && e.kind != "hackernews"
                && !suspicious(&e.material())
                && !e.title.trim().is_empty()
                && !e.excerpt.trim().is_empty()
                && e.title.chars().count() <= 512
                && e.excerpt.chars().count() <= 2000
                && source_matches(e)
        })
        .cloned()
        .collect();
    eligible.sort_by(|a, b| {
        b.weight
            .cmp(&a.weight)
            .then_with(|| a.published_at.cmp(&b.published_at))
            .then_with(|| a.article_id.cmp(&b.article_id))
    });
    if let Some(primary) = eligible.iter().find(|e| e.layer == "A" || e.weight == 5) {
        return Assessment {
            level: Level::Primary,
            reason: "official_source".into(),
            sources: vec![primary.clone()],
        };
    }
    let mut domains = BTreeSet::new();
    let mut origins = BTreeSet::new();
    let mut fingerprints = BTreeSet::new();
    let mut articles = BTreeSet::new();
    let mut independent = Vec::new();
    for evidence in eligible {
        let Some(host) = domain(&evidence.article_url) else {
            continue;
        };
        let Some(origin) = evidence
            .origin_group
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let origin = normalized_text(origin);
        let fingerprint = blake3::hash(normalized_text(&evidence.excerpt).as_bytes())
            .to_hex()
            .to_string();
        if domains.contains(&host)
            || origins.contains(&origin)
            || fingerprints.contains(&fingerprint)
            || articles.contains(&evidence.article_id)
        {
            continue;
        }
        domains.insert(host);
        origins.insert(origin);
        fingerprints.insert(fingerprint);
        articles.insert(evidence.article_id);
        independent.push(evidence);
    }
    if independent.len() >= 2 {
        Assessment {
            level: Level::Independent,
            reason: "independent_sources".into(),
            sources: independent.into_iter().take(8).collect(),
        }
    } else {
        observation("awaiting_confirmation")
    }
}
