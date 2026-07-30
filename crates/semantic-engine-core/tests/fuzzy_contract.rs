use semantic_engine_core::{
    AnswerTarget, Decision, EvidenceKind, Round, Submission, ValidationPolicy, Validator,
};

fn game_round() -> Round {
    Round {
        id: "round-games".into(),
        targets: vec![AnswerTarget {
            id: "elden-ring".into(),
            canonical: "Elden Ring".into(),
            aliases: Vec::new(),
        }],
        policy: ValidationPolicy::default(),
    }
}

fn submit(text: &str) -> semantic_engine_core::Validation {
    Validator::default().validate(
        &game_round(),
        &Submission {
            message_id: "msg-fuzzy".into(),
            participant_id: "viewer-fuzzy".into(),
            source_sequence: 8,
            text: text.into(),
        },
    )
}

#[test]
fn a_small_typo_in_a_game_title_is_accepted() {
    let validation = submit("eldern ring");

    assert_eq!(validation.decision, Decision::Accepted);
    assert_eq!(validation.target_id.as_deref(), Some("elden-ring"));
    assert!(validation.score >= 0.88);
    assert_eq!(validation.evidence[0].kind, EvidenceKind::FuzzyExpression);
}

#[test]
fn a_borderline_title_abstains_instead_of_guessing() {
    let validation = submit("elden kings");

    assert_eq!(validation.decision, Decision::Abstained);
    assert_eq!(validation.target_id.as_deref(), Some("elden-ring"));
    assert!(validation.score >= 0.72);
    assert!(validation.score < 0.88);
}

#[test]
fn an_unrelated_title_is_rejected() {
    let validation = submit("dark souls");

    assert_eq!(validation.decision, Decision::Rejected);
    assert_eq!(validation.target_id, None);
}
