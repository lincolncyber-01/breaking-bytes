//! Parsers limitados dos contratos de feeds e APIs públicas.

use crate::CollectorError;
use chrono::{DateTime, Utc};
use quick_xml::{Reader, events::Event};
use serde::Deserialize;
use stories_core::CapturedArticle;
use stories_normalize::html_metadata;
use url::Url;

pub fn xml_limits(bytes: &[u8]) -> Result<(), CollectorError> {
    let mut reader = Reader::from_reader(bytes);
    let mut depth = 0usize;
    let mut nodes = 0usize;
    loop {
        match reader.read_event().map_err(|_| CollectorError::Parse)? {
            Event::Start(_) => {
                depth += 1;
                nodes += 1;
                if depth > 64 || nodes > 20_000 {
                    return Err(CollectorError::Limits);
                }
            }
            Event::Empty(_) => {
                nodes += 1;
                if nodes > 20_000 {
                    return Err(CollectorError::Limits);
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
            }
            Event::DocType(_) => return Err(CollectorError::Parse),
            Event::Eof => {
                if depth != 0 {
                    return Err(CollectorError::Parse);
                }
                break;
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn feed(bytes: &[u8], max_items: usize) -> Result<Vec<CapturedArticle>, CollectorError> {
    xml_limits(bytes)?;
    let feed = feed_rs::parser::parse(bytes).map_err(|_| CollectorError::Parse)?;
    Ok(feed
        .entries
        .into_iter()
        .take(max_items)
        .filter_map(|entry| {
            let url = entry
                .links
                .iter()
                .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
                .map(|l| l.href.clone())?;
            let title = entry.title?.content;
            let excerpt = entry
                .content
                .and_then(|c| c.body)
                .or_else(|| entry.summary.map(|s| s.content))
                .unwrap_or_default();
            let canonical = Url::parse(&url)
                .ok()
                .and_then(|base| html_metadata(&base, &excerpt).ok())
                .map(|a| a.url)
                .unwrap_or(url);
            Some(CapturedArticle {
                url: canonical,
                title,
                excerpt,
                image_url: None,
                published_at: entry.published,
            })
        })
        .collect())
}

pub fn sitemap(bytes: &[u8], max_items: usize) -> Result<Vec<CapturedArticle>, CollectorError> {
    xml_limits(bytes)?;
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut field = String::new();
    let mut location = String::new();
    let mut title = String::new();
    let mut date = String::new();
    let mut articles = Vec::new();
    loop {
        match reader.read_event().map_err(|_| CollectorError::Parse)? {
            Event::Start(e) => field = e.name().as_ref().to_owned(),
            Event::Text(e) => {
                let text = e.xml10_content();
                match field.as_str() {
                    "loc" => location.push_str(&text),
                    "news:title" => title.push_str(&text),
                    "news:publication_date" => date.push_str(&text),
                    _ => {}
                }
            }
            Event::CData(e) => {
                let text = e.xml10_content();
                match field.as_str() {
                    "loc" => location.push_str(&text),
                    "news:title" => title.push_str(&text),
                    "news:publication_date" => date.push_str(&text),
                    _ => {}
                }
            }
            Event::GeneralRef(e) => {
                let entity = format!("&{};", e.as_ref());
                let text = htmlescape::decode_html(&entity).map_err(|_| CollectorError::Parse)?;
                match field.as_str() {
                    "loc" => location.push_str(&text),
                    "news:title" => title.push_str(&text),
                    _ => {}
                }
            }
            Event::End(e) if e.local_name().as_ref() == "url" => {
                if !location.is_empty() {
                    articles.push(CapturedArticle {
                        url: std::mem::take(&mut location),
                        title: std::mem::take(&mut title),
                        excerpt: String::new(),
                        image_url: None,
                        published_at: date_time(&date),
                    });
                }
                date.clear();
                field.clear();
                if articles.len() >= max_items {
                    break;
                }
            }
            Event::End(_) => field.clear(),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(articles)
}

fn date_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|v| v.with_timezone(&Utc))
}

#[derive(Deserialize)]
struct Release {
    html_url: String,
    name: Option<String>,
    tag_name: String,
    body: Option<String>,
    published_at: Option<DateTime<Utc>>,
    draft: bool,
}

pub fn github(bytes: &[u8], max_items: usize) -> Result<Vec<CapturedArticle>, CollectorError> {
    let entries: Vec<Release> = serde_json::from_slice(bytes).map_err(|_| CollectorError::Parse)?;
    Ok(entries
        .into_iter()
        .filter(|r| !r.draft)
        .take(max_items)
        .map(|r| CapturedArticle {
            url: r.html_url,
            title: r.name.filter(|n| !n.is_empty()).unwrap_or(r.tag_name),
            excerpt: r.body.unwrap_or_default(),
            image_url: None,
            published_at: r.published_at,
        })
        .collect())
}

#[derive(Deserialize)]
struct IncidentPage {
    incidents: Vec<Incident>,
}
#[derive(Deserialize)]
struct Incident {
    name: String,
    shortlink: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    incident_updates: Vec<IncidentUpdate>,
}
#[derive(Deserialize)]
struct IncidentUpdate {
    body: String,
}

pub fn status(bytes: &[u8], max_items: usize) -> Result<Vec<CapturedArticle>, CollectorError> {
    let page: IncidentPage = serde_json::from_slice(bytes).map_err(|_| CollectorError::Parse)?;
    Ok(page
        .incidents
        .into_iter()
        .take(max_items)
        .map(|i| CapturedArticle {
            url: i.shortlink,
            title: i.name,
            excerpt: i
                .incident_updates
                .into_iter()
                .next()
                .map(|u| u.body)
                .unwrap_or_default(),
            image_url: None,
            published_at: Some(i.created_at),
        })
        .collect())
}

#[derive(Deserialize)]
pub struct HnItem {
    pub id: u64,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub time: Option<i64>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub dead: bool,
}

pub fn hackernews(bytes: &[u8]) -> Result<Option<CapturedArticle>, CollectorError> {
    let entry: Option<HnItem> = serde_json::from_slice(bytes).map_err(|_| CollectorError::Parse)?;
    let Some(entry) = entry else {
        return Ok(None);
    };
    if entry.deleted || entry.dead || entry.kind.as_deref() != Some("story") {
        return Ok(None);
    }
    Ok(entry.title.map(|title| CapturedArticle {
        url: entry
            .url
            .unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={}", entry.id)),
        title,
        excerpt: String::new(),
        image_url: None,
        published_at: entry.time.and_then(|v| DateTime::from_timestamp(v, 0)),
    }))
}
