use semantic_engine_core::{
    AnswerTarget, Decision, Round, Submission, ValidationIssue, ValidationPolicy, Validator,
};

fn submission(text: String) -> Submission {
    Submission {
        message_id: "limit-message".into(),
        participant_id: "limit-viewer".into(),
        source_sequence: 1,
        text,
    }
}

#[test]
fn oversized_chat_input_is_rejected_even_when_it_matches_a_target() {
    let oversized = "a".repeat(1_001);
    let round = Round {
        id: "limit-round".into(),
        targets: vec![AnswerTarget {
            id: "oversized".into(),
            canonical: oversized.clone(),
            aliases: Vec::new(),
        }],
        policy: ValidationPolicy::default(),
    };

    let validation = Validator::default().validate(&round, &submission(oversized));

    assert_eq!(validation.decision, Decision::Rejected);
    assert_eq!(validation.target_id, None);
    assert_eq!(validation.issue, Some(ValidationIssue::InvalidSubmission));
}

#[test]
fn an_unbounded_alias_list_is_rejected_before_matching() {
    let round = Round {
        id: "limit-round".into(),
        targets: vec![AnswerTarget {
            id: "alias-bomb".into(),
            canonical: "safe title".into(),
            aliases: (0..65).map(|index| format!("alias-{index}")).collect(),
        }],
        policy: ValidationPolicy::default(),
    };

    let validation = Validator::default().validate(&round, &submission("alias-64".into()));

    assert_eq!(validation.decision, Decision::Rejected);
    assert_eq!(validation.target_id, None);
    assert_eq!(validation.issue, Some(ValidationIssue::InvalidRound));
}
