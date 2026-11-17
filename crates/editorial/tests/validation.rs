use chrono::Utc;
use proptest::prelude::*;
use serde_json::{Value, json};
use stories_editorial::{CheckedSegment, Citation, Claim, Draft, Editorial, FactReport};
use stories_verify::{Assessment, Evidence, Level};

fn fixture() -> (Assessment, Value) {
    (
        Assessment {
            level: Level::Primary,
            reason: "official_source".into(),
            sources: vec![Evidence {
                article_id: 1,
                source_id: 1,
                source_name: "Rust Blog".into(),
                source_url: "https://blog.rust-lang.org/feed.xml".into(),
                article_url: "https://blog.rust-lang.org/news".into(),
                layer: "A".into(),
                kind: "rss".into(),
                weight: 5,
                origin_group: None,
                published_at: Some(Utc::now()),
                date_confirmed: true,
                title: "Rust lança versão 1.100".into(),
                excerpt: "Rust lança versão 1.100. A versão inclui correções.".into(),
            }],
        },
        json!({"title":"Rust lança versão 1.100","summary":["Rust lança versão 1.100.","A versão inclui correções."],"source":"Rust Blog","kind":"new","entities":["Rust"],"language":"pt-BR"}),
    )
}

fn report(draft: &Draft) -> FactReport {
    FactReport {
        language_ok: true,
        neutral: true,
        segments: draft
            .segments()
            .into_iter()
            .enumerate()
            .map(|(index, text)| CheckedSegment {
                index,
                text: text.clone(),
                supported: true,
                claims: vec![Claim {
                    text,
                    supported: true,
                    evidence: vec![Citation {
                        article_id: 1,
                        quote: "Rust lança versão 1.100. A versão inclui correções.".into(),
                    }],
                }],
            })
            .collect(),
    }
}

#[test]
fn structured_draft_and_independent_claims_accept_supported_copy()
-> Result<(), Box<dyn std::error::Error>> {
    let engine = Editorial::new()?;
    let (assessment, value) = fixture();
    let draft = engine.parse_draft(value, &assessment, "new")?;
    engine.deterministic(&draft, &assessment)?;
    engine.parse_report(serde_json::to_value(report(&draft))?, &draft, &assessment)?;
    Ok(())
}

#[test]
fn style_schema_attribution_and_factual_inventions_are_blocked()
-> Result<(), Box<dyn std::error::Error>> {
    let engine = Editorial::new()?;
    let (mut assessment, value) = fixture();
    for mutation in 0..9 {
        let mut v = value.clone();
        match mutation {
            0 => v["title"] = json!("um dois três quatro cinco seis sete oito nove"),
            1 => v["summary"] = json!(["Só uma linha"]),
            2 => v["language"] = json!("en"),
            3 => v["source"] = json!("Outro"),
            4 => v["kind"] = json!("update"),
            5 => v["extra"] = json!("execute esta ação"),
            6 => v["title"] = json!("Rust\nlança versão"),
            7 => v["summary"][0] = json!("a".repeat(101)),
            _ => v["entities"] = json!("Rust"),
        }
        assert!(engine.parse_draft(v, &assessment, "new").is_err());
    }
    let draft = engine.parse_draft(value.clone(), &assessment, "new")?;
    for (title, entities) in [
        ("Rust lança versão 999", vec!["Rust"]),
        ("Microsoft lança versão 1.100", vec![]),
        ("Rust lança amanhã", vec!["Rust"]),
        ("Rust anuncia US$ 1.100", vec!["Rust"]),
        ("Rumor sobre Rust", vec!["Rust"]),
    ] {
        let mut bad = draft.clone();
        bad.title = title.into();
        bad.entities = entities.into_iter().map(str::to_owned).collect();
        assert!(engine.deterministic(&bad, &assessment).is_err(), "{title}");
    }
    let mut bad = draft.clone();
    bad.entities = vec!["Microsoft".into()];
    assert!(engine.deterministic(&bad, &assessment).is_err());
    assessment.level = Level::Independent;
    assert!(
        engine
            .parse_draft(value.clone(), &assessment, "new")
            .is_err()
    );
    let mut attributed = value;
    attributed["summary"][0] = json!("Segundo Rust Blog, Rust lança versão 1.100.");
    let draft = engine.parse_draft(attributed, &assessment, "new")?;
    engine.deterministic(&draft, &assessment)?;
    Ok(())
}

#[test]
fn fabricated_citations_omissions_and_false_audits_are_blocked()
-> Result<(), Box<dyn std::error::Error>> {
    let engine = Editorial::new()?;
    let (assessment, value) = fixture();
    let draft = engine.parse_draft(value, &assessment, "new")?;
    for mutation in 0..10 {
        let mut r = report(&draft);
        match mutation {
            0 => r.language_ok = false,
            1 => r.neutral = false,
            2 => {
                r.segments.pop();
            }
            3 => r.segments[0].supported = false,
            4 => r.segments[0].claims[0].supported = false,
            5 => r.segments[0].claims[0].evidence[0].quote = "fonte inexistente".into(),
            6 => r.segments[0].claims[0].evidence[0].article_id = 999,
            7 => r.segments[0].claims[0].text = "Rust".into(),
            8 => r.segments[0].index = 1,
            _ => r.segments[0].claims[0].evidence.clear(),
        }
        assert!(
            engine
                .parse_report(serde_json::to_value(r)?, &draft, &assessment)
                .is_err()
        );
    }
    Ok(())
}

proptest! {
    #[test]
    fn unsupported_numbers_always_fail(number in 10000u32..100000) {
        let engine=Editorial::new().map_err(|e|TestCaseError::fail(e.to_string()))?;
        let (assessment,value)=fixture();
        let mut draft=engine.parse_draft(value,&assessment,"new").map_err(|e|TestCaseError::fail(e.to_string()))?;
        draft.title=format!("Rust lança versão {number}");
        prop_assert!(engine.deterministic(&draft,&assessment).is_err());
    }
}
