use proptest::prelude::*;
use stories_core::{EventState, PublicationKey, PublicationKind};

#[test]
fn pipeline_requires_every_gate() -> Result<(), Box<dyn std::error::Error>> {
    let mut state = EventState::Detected;
    for next in [
        EventState::Clustered,
        EventState::Verified,
        EventState::Drafted,
        EventState::Checked,
        EventState::Rendered,
        EventState::Queued,
        EventState::Publishing,
        EventState::Published,
    ] {
        state = state.advance(next, None)?;
    }
    assert_eq!(state, EventState::Published);
    assert!(state.advance(EventState::NeedsCorrection, None).is_err());
    assert_eq!(
        state.advance(EventState::NeedsCorrection, Some("Fonte corrigiu a data"))?,
        EventState::NeedsCorrection
    );
    assert!(
        EventState::Detected
            .advance(EventState::Publishing, None)
            .is_err()
    );
    Ok(())
}

#[test]
fn terminal_states_cannot_be_reopened() {
    for state in EventState::ALL
        .into_iter()
        .filter(|state| state.is_terminal())
    {
        for next in EventState::ALL {
            assert!(state.advance(next, Some("auditoria")).is_err());
        }
    }
}

#[test]
fn state_names_match_storage() -> Result<(), Box<dyn std::error::Error>> {
    for state in EventState::ALL {
        assert_eq!(EventState::parse(state.as_str())?, state);
        assert_eq!(
            serde_json::from_str::<EventState>(&serde_json::to_string(&state)?)?,
            state
        );
    }
    assert!(EventState::parse("inventado").is_err());
    Ok(())
}

#[test]
fn rejection_requires_a_reason_and_preserves_published_events() {
    assert!(
        EventState::Verified
            .advance(EventState::Rejected, Some("  "))
            .is_err()
    );
    assert_eq!(
        EventState::Verified
            .advance(EventState::Rejected, Some("fonte retratou o anúncio"))
            .ok(),
        Some(EventState::Rejected)
    );
    assert!(
        EventState::Published
            .advance(EventState::Failed, Some("falha posterior"))
            .is_err()
    );
}

#[test]
fn publication_kinds_match_sql_values() -> Result<(), Box<dyn std::error::Error>> {
    for kind in [
        PublicationKind::New,
        PublicationKind::Update,
        PublicationKind::Correction,
    ] {
        assert_eq!(
            serde_json::to_string(&kind)?,
            format!("\"{}\"", kind.as_str())
        );
    }
    Ok(())
}

proptest! {
    #[test]
    fn no_state_can_transition_to_itself(index in 0usize..13) {
        let state = EventState::ALL[index];
        prop_assert!(state.advance(state, Some("auditoria")).is_err());
    }

    #[test]
    fn publication_keys_reject_invalid_ranges(event in i64::MIN..=0, seq in i32::MIN..0) {
        prop_assert!(PublicationKey::new(event, PublicationKind::New, 0).is_err());
        prop_assert!(PublicationKey::new(1, PublicationKind::Correction, seq).is_err());
    }

    #[test]
    fn publication_key_is_stable(event in 1i64..i64::MAX, seq in 0i32..i32::MAX) {
        let first = PublicationKey::new(event, PublicationKind::Update, seq);
        let second = PublicationKey::new(event, PublicationKind::Update, seq);
        prop_assert_eq!(first.as_ref().ok(), second.as_ref().ok());
        let key = first.map_err(|error| TestCaseError::fail(error.to_string()))?;
        prop_assert_eq!(key.event_id(), event);
        prop_assert_eq!(key.kind(), PublicationKind::Update);
        prop_assert_eq!(key.sequence(), seq);
    }
}
