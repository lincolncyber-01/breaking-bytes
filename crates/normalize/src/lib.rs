//! Canonicalização conservadora, limpeza de texto e extração determinística.

use chrono::{DateTime, Utc};
use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use stories_core::CapturedArticle;
use thiserror::Error;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedArticle {
    pub url: String,
    pub title: String,
    pub title_hash: [u8; 32],
    pub excerpt: String,
    pub image_url: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub entities: Vec<String>,
    pub facts: Vec<String>,
    pub tokens: Vec<String>,
}

pub fn canonical_url(input: &str) -> Result<Url, NormalizeError> {
    if input.len() > 8192 {
        return Err(NormalizeError::Limits);
    }
    let mut url = Url::parse(input).map_err(|_| NormalizeError::Url)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(NormalizeError::Url);
    }
    if url.scheme() == "http" {
        url.set_scheme("https").map_err(|_| NormalizeError::Url)?;
    }
    url.set_fragment(None);
    let mut query: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| {
            let key = key.to_ascii_lowercase();
            !key.starts_with("utm_")
                && !matches!(
                    key.as_str(),
                    "fbclid"
                        | "gclid"
                        | "dclid"
                        | "msclkid"
                        | "mc_cid"
                        | "mc_eid"
                        | "igshid"
                        | "ref_src"
                )
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    query.sort();
    url.set_query(None);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
}

pub fn normalized_text(text: &str) -> String {
    let decomposed: String = text
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    decomposed.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn plain_text(html: &str, max_chars: usize) -> Result<String, NormalizeError> {
    if html.len() > 2_097_152 || max_chars > 16_384 {
        return Err(NormalizeError::Limits);
    }
    let document = Html::parse_fragment(html);
    if document
        .tree
        .nodes()
        .any(|node| node.ancestors().take(66).count() > 64)
    {
        return Err(NormalizeError::Limits);
    }
    let ignored = selector("script,style,noscript,template")?;
    let ignored_ids: BTreeSet<_> = document
        .select(&ignored)
        .flat_map(|element| element.descendants().map(|n| n.id()))
        .collect();
    let raw = document
        .tree
        .nodes()
        .filter(|n| !ignored_ids.contains(&n.id()))
        .filter_map(|node| node.value().as_text())
        .map(|text| text.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let decoded = htmlescape::decode_html(&raw).unwrap_or(raw);
    Ok(decoded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect())
}

/// Segue canonical declarado e resolve imagens relativas, sem executar outra requisição.
pub fn html_metadata(base: &Url, body: &str) -> Result<CapturedArticle, NormalizeError> {
    if body.len() > 2_097_152 {
        return Err(NormalizeError::Limits);
    }
    let document = Html::parse_document(body);
    if document
        .tree
        .nodes()
        .any(|n| n.ancestors().take(66).count() > 64)
    {
        return Err(NormalizeError::Limits);
    }
    let canonical = document
        .select(&selector("link[rel='canonical']")?)
        .next()
        .and_then(|n| n.value().attr("href"))
        .map(|href| base.join(href).map_err(|_| NormalizeError::Url))
        .transpose()?
        .unwrap_or_else(|| base.clone());
    let title = meta(&document, "meta[property='og:title']")?
        .or_else(|| {
            document
                .select(&selector("title").ok()?)
                .next()
                .map(|n| n.text().collect::<String>())
        })
        .ok_or(NormalizeError::Title)?;
    let excerpt = meta(&document, "meta[name='description']")?.unwrap_or_default();
    let image_url = meta(&document, "meta[property='og:image']")?
        .and_then(|value| base.join(&value).ok())
        .map(|u| u.to_string());
    let published_at = meta(&document, "meta[property='article:published_time']")?
        .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
        .map(|v| v.with_timezone(&Utc));
    Ok(CapturedArticle {
        url: canonical_url(canonical.as_str())?.to_string(),
        title,
        excerpt,
        image_url,
        published_at,
    })
}

fn selector(value: &str) -> Result<Selector, NormalizeError> {
    Selector::parse(value).map_err(|_| NormalizeError::Selector)
}
fn meta(document: &Html, query: &str) -> Result<Option<String>, NormalizeError> {
    Ok(document
        .select(&selector(query)?)
        .next()
        .and_then(|n| n.value().attr("content"))
        .map(str::to_owned))
}

pub struct Normalizer {
    names: Regex,
    numbers: Regex,
}

impl Normalizer {
    pub fn new() -> Result<Self, NormalizeError> {
        Ok(Self {
            names: Regex::new(r"\b(?:[\p{Lu}][\p{L}\p{N}]+|iPhone|iPad|macOS|iOS|eBPF)\b").map_err(|_| NormalizeError::Pattern)?,
            numbers: Regex::new(r"(?i)(?:US\$|R\$|\$)?\s*\b\d+(?:[.,:/-]\d+)*(?:\s*%|\s*(?:bilhões|milhões|bilhão|milhão|million|billion))?").map_err(|_| NormalizeError::Pattern)?,
        })
    }

    pub fn normalize(&self, article: CapturedArticle) -> Result<NormalizedArticle, NormalizeError> {
        if article.title.len() > 16_384 || article.excerpt.len() > 2_097_152 {
            return Err(NormalizeError::Limits);
        }
        let title = plain_text(&article.title, 512)?;
        if title.trim().is_empty() {
            return Err(NormalizeError::Title);
        }
        let excerpt = plain_text(&article.excerpt, 2000)?;
        let hash = *blake3::hash(normalized_text(&title).as_bytes()).as_bytes();
        let combined = format!("{title} {excerpt}");
        let stop = [
            "a", "o", "the", "new", "novo", "nova", "segundo", "fonte", "breaking", "update",
        ];
        let entities = self
            .names
            .find_iter(&combined)
            .map(|m| normalized_text(m.as_str()))
            .filter(|n| !stop.contains(&n.as_str()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(64)
            .collect();
        let facts = self
            .numbers
            .find_iter(&combined)
            .map(|m| m.as_str().trim().to_lowercase())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(64)
            .collect();
        let tokens = normalized_text(&title)
            .split_whitespace()
            .filter(|t| t.len() > 2 && !t.chars().all(char::is_numeric))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(128)
            .collect();
        let image_url = article
            .image_url
            .and_then(|u| canonical_url(&u).ok())
            .map(|u| u.to_string());
        Ok(NormalizedArticle {
            url: canonical_url(&article.url)?.to_string(),
            title,
            title_hash: hash,
            excerpt,
            image_url,
            published_at: article.published_at,
            entities,
            facts,
            tokens,
        })
    }
}

#[derive(Debug, Error)]
pub enum NormalizeError {
    #[error("URL inválida ou contém credenciais")]
    Url,
    #[error("entrada excede os limites de normalização")]
    Limits,
    #[error("título ausente ou vazio")]
    Title,
    #[error("seletor interno inválido")]
    Selector,
    #[error("expressão interna inválida")]
    Pattern,
}
