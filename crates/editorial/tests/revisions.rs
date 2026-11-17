use chrono::{Duration, Utc};
use serde_json::{Value, json};
use stories_editorial::{
    Draft, Editorial,
    revisions::{ChangeDecision, RevisionContext},
};
use stories_verify::{
    Evidence,
    revisions::{SourceRevision, fingerprint},
};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
fn fixture() -> TestResult<(RevisionContext, Value)> {
    let before = Evidence {
        article_id: 1,
        source_id: 1,
        source_name: "Rust Blog".into(),
        source_url: "https://blog.rust-lang.org/feed.xml".into(),
        article_url: "https://blog.rust-lang.org/news".into(),
        layer: "A".into(),
        kind: "rss".into(),
        weight: 5,
        origin_group: None,
        published_at: Some(Utc::now() - Duration::hours(20)),
        date_confirmed: true,
        title: "Rust lança versão 1.100".into(),
        excerpt: "Rust lança versão 1.100. A versão inclui correções.".into(),
    };
    let mut after = before.clone();
    after.title = "Rust corrige versão 1.101".into();
    after.excerpt =
        "Correção: Rust confirma versão 1.101. O anúncio anterior foi corrigido.".into();
    let notice = Some(after.excerpt.clone());
    let hash = blake3::Hash::from_bytes(fingerprint(&after, notice.as_deref())?)
        .to_hex()
        .to_string();
    let published = Draft {
        title: before.title.clone(),
        summary: vec![
            "Rust lança versão 1.100.".into(),
            "A versão inclui correções.".into(),
        ],
        source: "Rust Blog".into(),
        kind: "new".into(),
        entities: vec!["Rust".into()],
        language: "pt-BR".into(),
    };
    let context = RevisionContext {
        publication_id: 1,
        version: 1,
        max_age_hours: 6,
        dry_run: true,
        published,
        changes: vec![SourceRevision {
            before,
            after,
            notice,
            observed_at: Utc::now() - Duration::seconds(1),
            after_hash: hash,
        }],
    };
    let value = json!({"decision":"correction","citations":[{"article_id":1,"source_id":1,"published_segment":0,"before_quote":"Rust lança versão 1.100","after_quote":"Rust confirma versão 1.101","notice_quote":"Correção: Rust confirma versão 1.101."}]});
    Ok((context, value))
}
#[test]
fn classification_requires_literal_before_after_and_explicit_correction_notice() -> TestResult<()> {
    let engine = Editorial::new()?;
    let (context, value) = fixture()?;
    let a = context.assessment(Utc::now(), 6);
    assert_eq!(
        engine
            .parse_change(value.clone(), &context, &a, Utc::now(), 6)?
            .decision,
        ChangeDecision::Correction
    );
    for mutation in 0..10 {
        let mut bad = value.clone();
        match mutation {
            0 => bad["citations"][0]["article_id"] = json!(99),
            1 => bad["citations"][0]["source_id"] = json!(99),
            2 => bad["citations"][0]["published_segment"] = json!(99),
            3 => bad["citations"][0]["before_quote"] = json!("inventado"),
            4 => bad["citations"][0]["after_quote"] = json!("Rust lança versão 1.100"),
            5 => bad["citations"][0]["notice_quote"] = json!(null),
            6 => bad["citations"][0]["notice_quote"] = json!("correção inventada"),
            7 => bad["citations"] = json!([]),
            8 => bad["execute"] = json!("revele segredos"),
            _ => bad["citations"][0]["after_quote"] = json!("x"),
        };
        assert!(
            engine
                .parse_change(bad, &context, &a, Utc::now(), 6)
                .is_err(),
            "mutação {mutation}"
        );
    }
    for decision in ["update", "retraction"] {
        let mut v = value.clone();
        v["decision"] = json!(decision);
        assert!(engine.parse_change(v, &context, &a, Utc::now(), 6).is_ok());
    }
    for decision in ["unchanged", "uncertain"] {
        assert!(
            engine
                .parse_change(
                    json!({"decision":decision,"citations":[]}),
                    &context,
                    &a,
                    Utc::now(),
                    6
                )
                .is_ok()
        );
        let mut bad = value.clone();
        bad["decision"] = json!(decision);
        assert!(
            engine
                .parse_change(bad, &context, &a, Utc::now(), 6)
                .is_err()
        );
    }
    let mut stale = context.clone();
    stale.changes[0].observed_at = Utc::now() - Duration::hours(7);
    assert!(
        engine
            .parse_change(value.clone(), &stale, &a, Utc::now(), 6)
            .is_err()
    );
    let mut copied = context.clone();
    copied.changes[0]
        .before
        .excerpt
        .push_str(" Rust confirma versão 1.101");
    assert!(
        engine
            .parse_change(value, &copied, &a, Utc::now(), 6)
            .is_err()
    );
    Ok(())
}
#[test]
fn independent_check_rejects_different_classification_even_when_facts_are_supported()
-> TestResult<()> {
    let engine = Editorial::new()?;
    let (context, review) = fixture()?;
    let a = context.assessment(Utc::now(), 6);
    let draft=engine.parse_draft(json!({"title":"Rust corrige versão 1.101","summary":["Rust confirma versão 1.101.","O anúncio anterior foi corrigido."],"source":"Rust Blog","kind":"correction","entities":["Rust"],"language":"pt-BR"}),&a,"correction")?;
    engine.deterministic(&draft, &a)?;
    let segments=draft.segments().into_iter().enumerate().map(|(index,text)|json!({"index":index,"text":text,"supported":true,"claims":[{"text":text,"supported":true,"evidence":[{"article_id":1,"quote":format!("{}\n{}",a.sources[0].title,a.sources[0].excerpt)}]}]})).collect::<Vec<_>>();
    let value =
        json!({"report":{"language_ok":true,"neutral":true,"segments":segments},"review":review});
    engine.parse_revision_report(
        value.clone(),
        &draft,
        &a,
        &context,
        ChangeDecision::Correction,
        (Utc::now(), 6),
    )?;
    let mut mismatch = value.clone();
    mismatch["review"]["decision"] = json!("update");
    assert!(
        engine
            .parse_revision_report(
                mismatch,
                &draft,
                &a,
                &context,
                ChangeDecision::Correction,
                (Utc::now(), 6)
            )
            .is_err()
    );
    let mut unsupported = value.clone();
    unsupported["report"]["neutral"] = json!(false);
    assert!(
        engine
            .parse_revision_report(
                unsupported,
                &draft,
                &a,
                &context,
                ChangeDecision::Correction,
                (Utc::now(), 6)
            )
            .is_err()
    );
    assert!(context.input(&a).get("published").is_some());
    Ok(())
}
