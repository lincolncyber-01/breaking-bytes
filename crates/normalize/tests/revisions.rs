use proptest::prelude::*;
use stories_normalize::{article_revision, revision_notice, revision_text};
use url::Url;

#[test]
fn late_editorial_notice_is_preserved_without_forging_publication_date()
-> Result<(), Box<dyn std::error::Error>> {
    let html = format!(
        "<title>Rust</title><meta property='article:published_time' content='2020-01-01T00:00:00Z'><meta property='article:modified_time' content='2026-11-17T00:00:00Z'><nav>Correction: falsa</nav><article>{}<p class='correction'>Correção: o saldo é -10, não 10.</p><script>Retraction: falsa</script></article>",
        "Texto anterior. ".repeat(300)
    );
    let revision = article_revision(&Url::parse("https://example.com/news")?, &html)?;
    assert_eq!(
        revision.notice.as_deref(),
        Some("Correção: o saldo é -10, não 10.")
    );
    assert!(revision.article.excerpt.starts_with("Correção:"));
    assert!(revision.article.excerpt.chars().count() <= 2000);
    assert!(!revision.article.excerpt.contains("falsa"));
    assert_eq!(
        revision.article.published_at.map(|d| d.to_rfc3339()),
        Some("2020-01-01T00:00:00+00:00".into())
    );
    Ok(())
}
#[test]
fn scripts_do_not_create_notices_and_lexical_notice_is_only_a_signal()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        revision_notice("<script>Correction: falso</script><style>Retraction: falso</style>Texto")?,
        None
    );
    assert_eq!(
        revision_notice("A fonte escreveu Correction: confirmar os números.")?.as_deref(),
        Some("Correction: confirmar os números.")
    );
    assert_eq!(revision_notice("Atualizado em 17/11/2026")?, None);
    let r = article_revision(
        &Url::parse("https://example.com")?,
        "<title>Rust</title><article>Correção: saldo -10.</article>",
    )?;
    assert_eq!(r.article.excerpt.matches("Correção:").count(), 1);
    assert!(revision_notice(&"x".repeat(2097153)).is_err());
    Ok(())
}
proptest! {
    #[test]
    fn comparison_ignores_only_whitespace_and_keeps_signs(n in 1i32..100000) {
        prop_assert_eq!(revision_text(&format!("saldo\t {n}\n")),format!("saldo {n}"));
        prop_assert_ne!(revision_text(&format!("saldo -{n}")),revision_text(&format!("saldo {n}")));
        prop_assert_eq!(revision_text("correc\u{327}a\u{303}o"),revision_text("correção"));
    }
}
