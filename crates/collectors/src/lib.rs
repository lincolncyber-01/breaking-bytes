//! Coleta assíncrona; parsing e extração HTML executam em spawn_blocking.

pub mod http;
mod parsers;
use chrono::{DateTime, Utc};
use http::{HttpClient, Validators};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use stories_core::CapturedArticle;
use thiserror::Error;
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Rss,
    Sitemap,
    Github,
    Edgar,
    Hackernews,
    Status,
}

impl SourceKind {
    pub fn parse(value: &str) -> Result<Self, CollectorError> {
        match value {
            "rss" => Ok(Self::Rss),
            "sitemap" => Ok(Self::Sitemap),
            "github" => Ok(Self::Github),
            "edgar" => Ok(Self::Edgar),
            "hackernews" => Ok(Self::Hackernews),
            "status" => Ok(Self::Status),
            _ => Err(CollectorError::Kind),
        }
    }
}

pub struct Batch {
    pub articles: Vec<CapturedArticle>,
    pub validators: Validators,
    pub not_modified: bool,
}

#[derive(Clone)]
pub struct Collector {
    http: HttpClient,
    max_items: usize,
    max_age_hours: u32,
    cpu: Arc<Semaphore>,
}

impl Collector {
    pub fn new(
        http: HttpClient,
        max_items: usize,
        max_age_hours: u32,
    ) -> Result<Self, CollectorError> {
        if !(1..=200).contains(&max_items) || !(1..=168).contains(&max_age_hours) {
            return Err(CollectorError::Limits);
        }
        Ok(Self {
            http,
            max_items,
            max_age_hours,
            cpu: Arc::new(Semaphore::new(2)),
        })
    }

    async fn cpu_work<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, CollectorError> + Send + 'static,
    ) -> Result<T, CollectorError> {
        let permit = self
            .cpu
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| CollectorError::Worker)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .map_err(|_| CollectorError::Worker)?
    }

    pub async fn collect(
        &self,
        kind: SourceKind,
        url: &str,
        validators: &Validators,
        now: DateTime<Utc>,
    ) -> Result<Batch, CollectorError> {
        let response = self.http.get(url, validators).await?;
        if response.status == 304 {
            return Ok(Batch {
                articles: Vec::new(),
                validators: response.validators,
                not_modified: true,
            });
        }
        if response.status != 200 {
            return Err(http::HttpError::Status {
                code: response.status,
                retry_after: None,
            }
            .into());
        }
        let body = response.body;
        let mut articles = if kind == SourceKind::Hackernews {
            let ids: Vec<u64> = self
                .cpu_work(move || serde_json::from_slice(&body).map_err(|_| CollectorError::Parse))
                .await?;
            let mut articles = Vec::new();
            for id in ids.into_iter().take(self.max_items) {
                let target = response
                    .url
                    .join(&format!("item/{id}.json"))
                    .map_err(|_| CollectorError::Parse)?;
                let item = self
                    .http
                    .get(target.as_str(), &Validators::default())
                    .await?;
                if item.status != 200 {
                    return Err(http::HttpError::Status {
                        code: item.status,
                        retry_after: None,
                    }
                    .into());
                }
                if let Some(article) = self
                    .cpu_work(move || parsers::hackernews(&item.body))
                    .await?
                {
                    articles.push(article);
                }
            }
            articles
        } else {
            let max_items = self.max_items;
            self.cpu_work(move || match kind {
                SourceKind::Rss | SourceKind::Edgar => parsers::feed(&body, max_items),
                SourceKind::Sitemap => parsers::sitemap(&body, max_items),
                SourceKind::Github => parsers::github(&body, max_items),
                SourceKind::Status => parsers::status(&body, max_items),
                SourceKind::Hackernews => Err(CollectorError::Kind),
            })
            .await?
        };
        articles.retain(|a| {
            (kind == SourceKind::Sitemap && a.published_at.is_none())
                || a.published_at.is_some_and(|date| {
                    date <= now + chrono::Duration::minutes(5)
                        && date >= now - chrono::Duration::hours(i64::from(self.max_age_hours))
                })
        });
        // Sitemaps comuns não têm manchete: buscar metadados respeitando robots e SSRF.
        if kind == SourceKind::Sitemap {
            for article in &mut articles {
                if article.title.is_empty() || article.published_at.is_none() {
                    let page = self.http.get(&article.url, &Validators::default()).await?;
                    if page.status != 200 {
                        return Err(http::HttpError::Status {
                            code: page.status,
                            retry_after: None,
                        }
                        .into());
                    }
                    let original_date = article.published_at;
                    let body = String::from_utf8(page.body).map_err(|_| CollectorError::Parse)?;
                    let base = page.url;
                    let mut enriched = self
                        .cpu_work(move || {
                            stories_normalize::html_metadata(&base, &body)
                                .map_err(|_| CollectorError::Parse)
                        })
                        .await?;
                    enriched.published_at = enriched.published_at.or(original_date);
                    *article = enriched;
                }
            }
        }
        articles.retain(|a| {
            a.published_at.is_some_and(|date| {
                date <= now + chrono::Duration::minutes(5)
                    && date >= now - chrono::Duration::hours(i64::from(self.max_age_hours))
            })
        });
        Ok(Batch {
            articles,
            validators: response.validators,
            not_modified: false,
        })
    }
}

#[derive(Debug, Error)]
pub enum CollectorError {
    #[error(transparent)]
    Http(#[from] http::HttpError),
    #[error("formato externo inválido")]
    Parse,
    #[error("limite de parsing excedido")]
    Limits,
    #[error("tipo de fonte desconhecido")]
    Kind,
    #[error("processamento isolado falhou")]
    Worker,
}

impl CollectorError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Http(_) => "http",
            Self::Parse => "parse",
            Self::Limits => "limits",
            Self::Kind => "kind",
            Self::Worker => "worker",
        }
    }
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Self::Http(e) => e.retry_after(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
