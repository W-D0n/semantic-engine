use semantic_engine_core::{
    AnswerTarget, Decision, EvidenceKind, Round, Submission, ValidationPolicy, Validator,
};

fn movie_round() -> Round {
    Round {
        id: "round-1".into(),
        targets: vec![AnswerTarget {
            id: "spirited-away".into(),
            canonical: "Spirited Away".into(),
            aliases: vec!["Le Voyage de Chihiro".into()],
        }],
        policy: ValidationPolicy::default(),
    }
}

#[test]
fn a_configured_french_title_is_accepted_and_keeps_workflow_identity() {
    let validation = Validator::default().validate(
        &movie_round(),
        &Submission {
            message_id: "msg-42".into(),
            participant_id: "viewer-7".into(),
            source_sequence: 108,
            text: "Le Voyage de Chihiro".into(),
        },
    );

    assert_eq!(validation.decision, Decision::Accepted);
    assert_eq!(validation.round_id, "round-1");
    assert_eq!(validation.message_id, "msg-42");
    assert_eq!(validation.participant_id, "viewer-7");
    assert_eq!(validation.source_sequence, 108);
    assert_eq!(validation.target_id.as_deref(), Some("spirited-away"));
    assert_eq!(validation.score, 1.0);
}

#[test]
fn case_accents_spacing_and_punctuation_do_not_change_the_answer() {
    let validation = Validator::default().validate(
        &movie_round(),
        &Submission {
            message_id: "msg-43".into(),
            participant_id: "viewer-8".into(),
            source_sequence: 109,
            text: "  LE   VOYAGE DE CHIHIRO !!! ".into(),
        },
    );

    assert_eq!(validation.decision, Decision::Accepted);
    assert_eq!(validation.target_id.as_deref(), Some("spirited-away"));
    assert_eq!(validation.score, 1.0);
    assert_eq!(validation.evidence[0].kind, EvidenceKind::NormalizedExpression);
}
