use proptest::prelude::*;
use stories_cluster::{Match, classify, similarity};
use stories_core::CapturedArticle;
use stories_normalize::Normalizer;

#[test]
fn separates_unrelated_announcements_and_marks_new_facts() -> Result<(), Box<dyn std::error::Error>>
{
    let n = Normalizer::new()?;
    let make = |title: &str| {
        n.normalize(CapturedArticle {
            url: "https://example.com/a".into(),
            title: title.into(),
            excerpt: String::new(),
            image_url: None,
            published_at: None,
        })
    };
    let a = make("Rust anuncia versão 1.100")?;
    let same = make("Rust anuncia versão 1.100!")?;
    let update = make("Rust anuncia versão 1.101")?;
    let other = make("Rust corrige problema de memória")?;
    assert_eq!(classify(&a, &same, 0.86), Match::Same);
    assert_eq!(classify(&a, &update, 0.86), Match::NewFacts);
    assert_eq!(classify(&a, &other, 0.86), Match::Different);
    Ok(())
}

proptest! {
    #[test]
    fn formatting_changes_do_not_create_a_new_event(title in "[A-Z][a-z]{2,20} [a-z]{3,20} [0-9]{1,4}") {
        let n=Normalizer::new().map_err(|e|TestCaseError::fail(e.to_string()))?;
        let make=|text:String|n.normalize(CapturedArticle{url:"https://example.com/a".into(),title:text,excerpt:String::new(),image_url:None,published_at:None});
        let a=make(title.clone()).map_err(|e|TestCaseError::fail(e.to_string()))?;
        let b=make(format!("{}!",title.to_uppercase())).map_err(|e|TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(classify(&a,&b,0.86),Match::Same);
    }
    #[test]
    fn similarity_is_symmetric_and_bounded(a in prop::collection::vec("[a-z]{1,8}",0..30), b in prop::collection::vec("[a-z]{1,8}",0..30)) {
        let score=similarity(&a,&b);
        prop_assert!((0.0..=1.0).contains(&score));
        prop_assert_eq!(score,similarity(&b,&a));
    }
}
