use chrono::{Duration, Utc};
use proptest::prelude::*;
use stories_verify::{
    Evidence, Level,
    revisions::{SourceRevision, assess, fingerprint},
};

fn revision(id: i64, host: &str) -> Result<SourceRevision, Box<dyn std::error::Error>> {
    let before = Evidence {
        article_id: id,
        source_id: id as i32,
        source_name: host.into(),
        source_url: format!("https://{host}/feed"),
        article_url: format!("https://{host}/article"),
        layer: "B".into(),
        kind: "rss".into(),
        weight: 3,
        origin_group: Some(host.into()),
        published_at: Some(Utc::now() - Duration::hours(20)),
        date_confirmed: true,
        title: "Rust anuncia saldo 10".into(),
        excerpt: format!("Saldo de Rust é 10. Reportagem {host}."),
    };
    let mut after = before.clone();
    after.excerpt = format!("Correção: saldo de Rust é -10. Reportagem {host}.");
    let notice = Some(after.excerpt.clone());
    let hash = fingerprint(&after, notice.as_deref())?;
    Ok(SourceRevision {
        before,
        after,
        notice,
        observed_at: Utc::now() - Duration::seconds(1),
        after_hash: blake3::Hash::from_bytes(hash).to_hex().to_string(),
    })
}
#[test]
fn revisions_require_fresh_observation_unchanged_identity_and_matching_hash()
-> Result<(), Box<dyn std::error::Error>> {
    let good = revision(1, "example.com")?;
    let now = Utc::now();
    assert!(good.eligible(now, 6));
    assert!(good.changed());
    assert_eq!(good.before.published_at, good.after.published_at);
    for mutation in 0..9 {
        let mut r = good.clone();
        match mutation {
            0 => r.observed_at = now - Duration::hours(7),
            1 => r.observed_at = now + Duration::seconds(1),
            2 => r.after.source_id = 2,
            3 => r.after.source_url = "https://other.example.com".into(),
            4 => r.after.excerpt.push('!'),
            5 => r.after.layer = "C".into(),
            6 => r.notice = Some("Inventada".into()),
            7 => r.after.published_at = Some(now + Duration::hours(1)),
            _ => r.after.title.clear(),
        };
        assert!(!r.eligible(now, 6), "mutação {mutation}");
    }
    assert!(!good.eligible(now, 0));
    assert!(!good.eligible(now, 169));
    let mut missing_date = good.clone();
    missing_date.after.published_at = None;
    missing_date.after_hash = blake3::Hash::from_bytes(fingerprint(
        &missing_date.after,
        missing_date.notice.as_deref(),
    )?)
    .to_hex()
    .to_string();
    assert!(missing_date.eligible(now, 6));
    Ok(())
}
#[test]
fn n2_requires_independent_sources_and_n1_accepts_one_primary()
-> Result<(), Box<dyn std::error::Error>> {
    let mut a = revision(1, "example.com")?;
    let b = revision(2, "example.net")?;
    let now = Utc::now();
    assert_eq!(assess(&[a.clone()], now, 6).level, Level::Observation);
    assert_eq!(
        assess(&[a.clone(), b.clone()], now, 6).level,
        Level::Independent
    );
    for mutation in 0..4 {
        let mut copy = b.clone();
        match mutation {
            0 => copy.after.origin_group = a.after.origin_group.clone(),
            1 => copy.after.excerpt = a.after.excerpt.clone(),
            2 => copy.after.article_url = "https://sub.example.com/article".into(),
            _ => copy.after.article_id = a.after.article_id,
        };
        copy.before.origin_group = copy.after.origin_group.clone();
        copy.before.article_url = copy.after.article_url.clone();
        copy.before.article_id = copy.after.article_id;
        copy.notice = None;
        copy.after_hash = blake3::Hash::from_bytes(fingerprint(&copy.after, None)?)
            .to_hex()
            .to_string();
        assert_eq!(assess(&[a.clone(), copy], now, 6).level, Level::Observation);
    }
    a.before.layer = "A".into();
    a.after.layer = "A".into();
    a.after_hash = blake3::Hash::from_bytes(fingerprint(&a.after, a.notice.as_deref())?)
        .to_hex()
        .to_string();
    assert_eq!(assess(&[a], now, 6).level, Level::Primary);
    assert_eq!(assess(&[], now, 6).level, Level::Observation);
    Ok(())
}
proptest! {
    #[test]
    fn hash_keeps_numeric_sign_and_ignores_observation_metadata(n in 1i32..99999) {
        let mut r=revision(1,"example.com").map_err(|e|TestCaseError::fail(e.to_string()))?;
        r.after.excerpt=format!("Saldo {n}");let positive=fingerprint(&r.after,None).map_err(|e|TestCaseError::fail(e.to_string()))?;
        r.after.excerpt=format!("Saldo -{n}");let negative=fingerprint(&r.after,None).map_err(|e|TestCaseError::fail(e.to_string()))?;
        prop_assert_ne!(positive,negative);r.after.date_confirmed=false;
        prop_assert_eq!(negative,fingerprint(&r.after,None).map_err(|e|TestCaseError::fail(e.to_string()))?);
        r.after.excerpt=format!("Saldo\n  -{n} ");prop_assert_eq!(negative,fingerprint(&r.after,None).map_err(|e|TestCaseError::fail(e.to_string()))?);
    }
}
