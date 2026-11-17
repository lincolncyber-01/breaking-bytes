//! Fallback conservador previsto no blueprint: hash, entidades e fatos.

use std::collections::BTreeSet;
use stories_normalize::NormalizedArticle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    Different,
    Same,
    NewFacts,
}

pub fn classify(left: &NormalizedArticle, right: &NormalizedArticle, threshold: f64) -> Match {
    if !(0.0..=1.0).contains(&threshold) {
        return Match::Different;
    }
    let same_hash = left.title_hash == right.title_hash;
    let shared_entity = left
        .entities
        .iter()
        .any(|entity| right.entities.contains(entity));
    let similar = shared_entity && similarity(&left.tokens, &right.tokens) >= threshold;
    if !same_hash && !similar {
        return Match::Different;
    }
    let new_facts = right.facts.iter().any(|fact| !left.facts.contains(fact));
    if new_facts {
        Match::NewFacts
    } else {
        Match::Same
    }
}

pub fn similarity(left: &[String], right: &[String]) -> f64 {
    let left: BTreeSet<_> = left.iter().collect();
    let right: BTreeSet<_> = right.iter().collect();
    let union = left.union(&right).count();
    if union == 0 {
        return 0.0;
    }
    left.intersection(&right).count() as f64 / union as f64
}
