use chrono::{Duration, Utc};
use proptest::prelude::*;
use stories_verify::{Evidence, Level, assess, domain, source_matches, suspicious};

fn evidence(id: i64, site: &str) -> Evidence {
    Evidence {
        article_id: id,
        source_id: id as i32,
        source_name: site.into(),
        source_url: format!("https://{site}/feed"),
        article_url: format!("https://{site}/news"),
        layer: "B".into(),
        kind: "rss".into(),
        weight: 3,
        origin_group: Some(site.into()),
        published_at: Some(Utc::now() - Duration::minutes(1)),
        date_confirmed: true,
        title: "Rust lança versão 1.100".into(),
        excerpt: format!("Rust publica correções. Reportagem própria de {site}. "),
    }
}

#[test]
fn levels_require_dates_independence_and_no_signals() {
    let now = Utc::now();
    let mut a = evidence(1, "example.com");
    let b = evidence(2, "example.net");
    assert_eq!(assess(&[a.clone()], now, 6).level, Level::Observation);
    assert_eq!(
        assess(&[a.clone(), b.clone()], now, 6).level,
        Level::Independent
    );
    a.weight = 5;
    assert_eq!(assess(&[a.clone()], now, 6).level, Level::Primary);
    for mutation in 0..7 {
        let mut invalid = a.clone();
        match mutation {
            0 => invalid.layer = "C".into(),
            1 => invalid.kind = "hackernews".into(),
            2 => invalid.date_confirmed = false,
            3 => invalid.published_at = None,
            4 => invalid.published_at = Some(now - Duration::hours(7)),
            5 => invalid.published_at = Some(now + Duration::hours(1)),
            _ => invalid.title = "Rumor sobre Rust".into(),
        }
        assert_eq!(assess(&[invalid], now, 6).level, Level::Observation);
    }
    for mutation in 0..5 {
        let mut copy = b.clone();
        match mutation {
            0 => copy.excerpt = a.excerpt.clone(),
            1 => copy.origin_group = Some("EXAMPLE.COM".into()),
            2 => copy.origin_group = None,
            3 => copy.article_id = a.article_id,
            _ => copy.article_url = "https://other.example.com/news".into(),
        }
        a.weight = 3;
        assert_eq!(assess(&[a.clone(), copy], now, 6).level, Level::Observation);
    }
    assert_eq!(assess(&[], now, 6).reason, "insufficient_evidence");
    assert_eq!(assess(&[a.clone()], now, 0).level, Level::Observation);
    a.layer = "A".into();
    a.weight = 1;
    assert_eq!(assess(&[a], now, 6).level, Level::Primary);
}

#[test]
fn registrable_domains_and_repository_boundaries() {
    assert_eq!(
        domain("https://a.publisher.co.uk/a"),
        Some("publisher.co.uk".into())
    );
    assert_eq!(
        domain("https://b.publisher.co.uk/b"),
        Some("publisher.co.uk".into())
    );
    assert_eq!(domain("http://user:secret@example.com"), None);
    assert_eq!(domain("file:///etc/passwd"), None);
    let mut a = evidence(1, "example.com");
    a.kind = "github".into();
    a.source_url = "https://api.github.com/repos/rust-lang/rust/releases".into();
    a.article_url = "https://github.com/rust-lang/rust/releases/tag/1.100".into();
    assert!(source_matches(&a));
    a.article_url = "https://github.com/attacker/rust/releases/tag/1.100".into();
    assert!(!source_matches(&a));
    assert!(suspicious("Suposta compra sem confirmação"));
}

proptest! {
    #[test]
    fn duplicates_never_create_corroboration(copies in 1usize..32) {
        let a=evidence(1,"example.com");
        prop_assert_eq!(assess(&vec![a;copies],Utc::now(),6).level,Level::Observation);
    }
    #[test]
    fn any_unconfirmed_primary_stays_in_observation(weight in 1i16..=5) {
        let mut a=evidence(1,"example.com");a.layer="A".into();a.weight=weight;a.date_confirmed=false;
        prop_assert_eq!(assess(&[a],Utc::now(),6).level,Level::Observation);
    }
}
