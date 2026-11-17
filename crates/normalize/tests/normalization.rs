use proptest::prelude::*;
use stories_core::CapturedArticle;
use stories_normalize::{Normalizer, canonical_url, html_metadata, plain_text};

#[test]
fn canonicalization_preserves_article_parameters() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        canonical_url("http://EXAMPLE.com/news?id=7&utm_source=x&fbclid=y#part")?.as_str(),
        "https://example.com/news?id=7"
    );
    assert!(canonical_url("https://user:password@example.com/").is_err());
    assert!(canonical_url("file:///etc/passwd").is_err());
    Ok(())
}

#[test]
fn html_canonical_metadata_and_invisible_content() -> Result<(), Box<dyn std::error::Error>> {
    let base = canonical_url("https://example.com/news")?;
    let article = html_metadata(
        &base,
        r#"<html><head><title>Ação em Rust</title><link rel="canonical" href="/oficial?utm_x=a"><meta property="og:image" content="/image.jpg"></head></html>"#,
    )?;
    assert_eq!(article.url, "https://example.com/oficial");
    assert_eq!(
        article.image_url.as_deref(),
        Some("https://example.com/image.jpg")
    );
    assert_eq!(
        plain_text("<p>ação &amp; dados</p><script>instrução</script>", 100)?,
        "ação & dados"
    );
    assert!(
        plain_text(
            &format!("{}x{}", "<div>".repeat(70), "</div>".repeat(70)),
            100
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn extraction_keeps_versions_and_accent_equivalence() -> Result<(), Box<dyn std::error::Error>> {
    let normalizer = Normalizer::new()?;
    let article = |title: &str| CapturedArticle {
        url: "https://example.com/a".into(),
        title: title.into(),
        excerpt: String::new(),
        image_url: None,
        published_at: None,
    };
    let a = normalizer.normalize(article("Rust anuncia versão 1.100"))?;
    let b = normalizer.normalize(article("RUST anuncia versao 1.100!"))?;
    assert_eq!(a.title_hash, b.title_hash);
    assert!(a.entities.contains(&"rust".to_owned()));
    assert!(a.facts.contains(&"1.100".to_owned()));
    Ok(())
}

proptest! {
    #[test]
    fn canonicalization_is_idempotent(path in "[a-zA-Z0-9/_-]{0,80}", value in "[a-z0-9]{0,20}") {
        let first=canonical_url(&format!("http://EXAMPLE.com/{path}?id={value}&utm_source=test"))
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let second=canonical_url(first.as_str()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(first,second);
    }
}
