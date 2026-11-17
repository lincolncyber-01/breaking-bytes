use super::*;
use std::net::IpAddr;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[tokio::test]
async fn transient_failures_timeout_and_redirect_rules() -> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    Mock::given(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /private\n"),
        )
        .mount(&server)
        .await;
    Mock::given(path("/forward"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/private"))
        .mount(&server)
        .await;
    Mock::given(path("/private"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(path("/loop"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
        .mount(&server)
        .await;
    Mock::given(path("/failure"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "60"))
        .mount(&server)
        .await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(2)))
        .mount(&server)
        .await;
    let http = HttpClient::local(&server.uri(), 8192)?;
    assert!(matches!(
        http.get(&format!("{}/forward", server.uri()), &Validators::default())
            .await,
        Err(http::HttpError::Robots)
    ));
    assert!(matches!(
        http.get(&format!("{}/loop", server.uri()), &Validators::default())
            .await,
        Err(http::HttpError::Redirect)
    ));
    let error = http
        .get(&format!("{}/failure", server.uri()), &Validators::default())
        .await
        .err()
        .ok_or("HTTP 503 ausente")?;
    assert_eq!(error.retry_after(), Some(60));
    assert!(
        http.get(&format!("{}/slow", server.uri()), &Validators::default())
            .await
            .is_err()
    );
    Ok(())
}

#[test]
fn atom_contract_matches_edgar_and_regular_feeds() -> Result<(), Box<dyn std::error::Error>> {
    let atom=br#"<feed xmlns="http://www.w3.org/2005/Atom"><id>https://example.com</id><title>News</title><updated>2026-11-16T12:00:00Z</updated><entry><id>https://example.com/a</id><title>Rust</title><link href="https://example.com/a"/><published>2026-11-16T12:00:00Z</published><updated>2026-11-16T12:30:00Z</updated></entry></feed>"#;
    let entries = parsers::feed(atom, 10)?;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].published_at,
        Some(DateTime::parse_from_rfc3339("2026-11-16T12:00:00Z")?.with_timezone(&Utc))
    );
    Ok(())
}

fn now() -> Result<DateTime<Utc>, Box<dyn std::error::Error>> {
    Ok(DateTime::parse_from_rfc3339("2026-11-16T13:00:00Z")?.with_timezone(&Utc))
}

#[tokio::test]
async fn feed_contract_conditional_requests_and_age_filter()
-> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    let body = r#"<rss version="2.0"><channel><title>Tech</title><link>https://example.com</link><description>News</description><item><title>Rust 1.100</title><link>https://example.com/rust</link><pubDate>Mon, 16 Nov 2026 12:00:00 GMT</pubDate><description>Fonte oficial</description></item><item><title>Antigo</title><link>https://example.com/old</link><pubDate>Mon, 16 Nov 2020 12:00:00 GMT</pubDate></item></channel></rss>"#;
    Mock::given(method("GET"))
        .and(path("/feed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("etag", "\"v1\""),
        )
        .expect(1)
        .mount(&server)
        .await;
    let collector = Collector::new(HttpClient::local(&server.uri(), 8192)?, 20, 6)?;
    let batch = collector
        .collect(
            SourceKind::Rss,
            &format!("{}/feed", server.uri()),
            &Validators::default(),
            now()?,
        )
        .await?;
    assert_eq!(batch.articles.len(), 1);
    assert_eq!(batch.validators.etag.as_deref(), Some("\"v1\""));
    server.reset().await;
    Mock::given(path("/feed"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    let batch = collector
        .collect(
            SourceKind::Rss,
            &format!("{}/feed", server.uri()),
            &batch.validators,
            now()?,
        )
        .await?;
    assert!(batch.not_modified);
    assert!(batch.articles.is_empty());
    Ok(())
}

#[tokio::test]
async fn robots_limits_rate_limit_and_redirect_contracts() -> Result<(), Box<dyn std::error::Error>>
{
    let server = MockServer::start().await;
    Mock::given(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /private\n"),
        )
        .mount(&server)
        .await;
    Mock::given(path("/large"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(4096)))
        .mount(&server)
        .await;
    Mock::given(path("/limited"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120"))
        .mount(&server)
        .await;
    Mock::given(path("/redirect"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", "http://169.254.169.254/latest/meta-data"),
        )
        .mount(&server)
        .await;
    let http = HttpClient::local(&server.uri(), 1024)?;
    assert!(matches!(
        http.get(&format!("{}/private", server.uri()), &Validators::default())
            .await,
        Err(http::HttpError::Robots)
    ));
    assert!(matches!(
        http.get(&format!("{}/large", server.uri()), &Validators::default())
            .await,
        Err(http::HttpError::Limits)
    ));
    let error = http
        .get(&format!("{}/limited", server.uri()), &Validators::default())
        .await
        .err()
        .ok_or("rate limit ausente")?;
    assert_eq!(error.retry_after(), Some(120));
    assert!(matches!(
        http.get(
            &format!("{}/redirect", server.uri()),
            &Validators::default()
        )
        .await,
        Err(http::HttpError::Ssrf)
    ));
    Ok(())
}

#[test]
fn ssrf_blocks_special_networks_and_literal_bypasses() -> Result<(), Box<dyn std::error::Error>> {
    for ip in [
        "127.0.0.1",
        "10.0.0.1",
        "100.64.1.1",
        "169.254.169.254",
        "192.168.1.1",
        "198.18.0.1",
        "224.0.0.1",
        "::1",
        "fc00::1",
        "fe80::1",
        "::ffff:127.0.0.1",
        "2001:db8::1",
        "2002:7f00:1::",
    ] {
        assert!(!http::public_ip(ip.parse::<IpAddr>()?));
    }
    assert!(http::public_ip("1.1.1.1".parse()?));
    assert!(http::public_ip("2606:4700:4700::1111".parse()?));
    for url in [
        "http://2130706433/",
        "http://127.1/",
        "http://[::1]/",
        "https://user:secret@example.com/",
        "file:///etc/passwd",
    ] {
        assert!(http::validate_url(&url::Url::parse(url)?).is_err());
    }
    Ok(())
}

#[test]
fn json_and_xml_contracts_reject_malformed_input() -> Result<(), Box<dyn std::error::Error>> {
    let releases=br#"[{"html_url":"https://github.com/a/b/releases/tag/v1","name":"v1","tag_name":"v1","body":"Release","published_at":"2026-11-16T12:00:00Z","draft":false}]"#;
    assert_eq!(parsers::github(releases, 10)?.len(), 1);
    let status=br#"{"incidents":[{"name":"Incidente","shortlink":"https://status.example.com/a","created_at":"2026-11-16T12:00:00Z","incident_updates":[{"body":"Investigando"}]}]}"#;
    assert_eq!(parsers::status(status, 10)?[0].excerpt, "Investigando");
    assert!(parsers::hackernews(br#"{"id":1,"type":"story","title":"Rumor","url":"https://example.com","time":1,"dead":true}"#)?.is_none());
    let sitemap=br#"<urlset xmlns:news="http://www.google.com/schemas/sitemap-news/0.9"><url><loc>https://example.com/a</loc><news:news><news:publication_date>2026-11-16T12:00:00Z</news:publication_date><news:title>A &amp; B</news:title></news:news></url></urlset>"#;
    assert_eq!(parsers::sitemap(sitemap, 10)?[0].title, "A & B");
    assert!(
        parsers::feed(
            b"<!DOCTYPE a [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><a/>",
            10
        )
        .is_err()
    );
    assert!(parsers::github(b"not json", 10).is_err());
    assert!(
        parsers::xml_limits(format!("{}{}", "<x>".repeat(70), "</x>".repeat(70)).as_bytes())
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn hackernews_uses_item_endpoint_and_filters_dead_items()
-> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    Mock::given(path("/v0/newstories.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([1, 2])))
        .mount(&server)
        .await;
    Mock::given(path("/v0/item/1.json")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":1,"type":"story","title":"Rust","url":"https://example.com/rust","time":1794830400}))).mount(&server).await;
    Mock::given(path("/v0/item/2.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"id":2,"type":"story","title":"Dead","time":1794830400,"dead":true}),
        ))
        .mount(&server)
        .await;
    let collector = Collector::new(HttpClient::local(&server.uri(), 8192)?, 10, 6)?;
    let batch = collector
        .collect(
            SourceKind::Hackernews,
            &format!("{}/v0/newstories.json", server.uri()),
            &Validators::default(),
            now()?,
        )
        .await?;
    assert_eq!(batch.articles.len(), 1);
    Ok(())
}
