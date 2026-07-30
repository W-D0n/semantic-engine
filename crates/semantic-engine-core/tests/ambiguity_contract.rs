use semantic_engine_core::{
    AnswerTarget, Decision, Round, Submission, ValidationPolicy, Validator,
};

fn submission(text: &str) -> Submission {
    Submission {
        message_id: "ambiguity-message".into(),
        participant_id: "ambiguity-viewer".into(),
        source_sequence: 4,
        text: text.into(),
    }
}

#[test]
fn a_shared_exact_alias_abstains_instead_of_using_target_order() {
    let round = Round {
        id: "ambiguous-round".into(),
        targets: vec![
            AnswerTarget {
                id: "elden-ring".into(),
                canonical: "Elden Ring".into(),
                aliases: vec!["ER".into()],
            },
            AnswerTarget {
                id: "eternal-return".into(),
                canonical: "Eternal Return".into(),
                aliases: vec!["ER".into()],
            },
        ],
        policy: ValidationPolicy::default(),
    };

    let validation = Validator::default().validate(&round, &submission("er"));

    assert_eq!(validation.decision, Decision::Abstained);
    assert_eq!(validation.target_id, None);
}

#[test]
fn an_unconfigured_fragment_is_not_promoted_to_an_exact_match() {
    let round = Round {
        id: "fragment-round".into(),
        targets: vec![AnswerTarget {
            id: "resident-evil-4".into(),
            canonical: "Resident Evil 4".into(),
            aliases: Vec::new(),
        }],
        policy: ValidationPolicy::default(),
    };

    let validation = Validator::default().validate(&round, &submission("resident"));

    assert_ne!(validation.decision, Decision::Accepted);
    assert!(validation.score < 0.87);
}
