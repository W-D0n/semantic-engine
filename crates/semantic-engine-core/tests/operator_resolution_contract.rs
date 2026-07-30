use semantic_engine_core::{
    AnswerTarget, Decision, OperatorResolutionRequest, ResolutionVerdict, Round, Submission,
    ValidationPolicy, Validator, resolve_validation,
};

#[test]
fn operator_can_resolve_an_abstention_without_erasing_the_engine_decision() {
    let round = Round {
        id: "live-round".to_owned(),
        targets: vec![AnswerTarget {
            id: "elden-ring".to_owned(),
            canonical: "Elden Ring".to_owned(),
            aliases: vec!["ER".to_owned()],
        }],
        policy: ValidationPolicy::default(),
    };
    let validation = Validator::default().validate(
        &round,
        &Submission {
            message_id: "chat-42".to_owned(),
            participant_id: "viewer-07".to_owned(),
            source_sequence: 42,
            text: "elden kings".to_owned(),
        },
    );
    assert_eq!(validation.decision, Decision::Abstained);

    let resolution = resolve_validation(
        &round,
        &validation,
        OperatorResolutionRequest {
            round_id: "live-round".to_owned(),
            message_id: "chat-42".to_owned(),
            verdict: ResolutionVerdict::Accepted,
            target_id: Some("elden-ring".to_owned()),
            note: "Faute comprise pendant le live".to_owned(),
        },
    )
    .expect("a valid operator resolution must be emitted");

    assert_eq!(resolution.original_decision, Decision::Abstained);
    assert_eq!(resolution.final_decision, Decision::Accepted);
    assert_eq!(resolution.target_id.as_deref(), Some("elden-ring"));
    assert_eq!(resolution.participant_id, "viewer-07");
    assert_eq!(resolution.source_sequence, 42);
}

#[test]
fn operator_resolution_rejects_a_target_outside_the_round() {
    let round = Round {
        id: "live-round".to_owned(),
        targets: vec![AnswerTarget {
            id: "elden-ring".to_owned(),
            canonical: "Elden Ring".to_owned(),
            aliases: vec![],
        }],
        policy: ValidationPolicy::default(),
    };
    let validation = Validator::default().validate(
        &round,
        &Submission {
            message_id: "chat-43".to_owned(),
            participant_id: "viewer-08".to_owned(),
            source_sequence: 43,
            text: "elden".to_owned(),
        },
    );

    let result = resolve_validation(
        &round,
        &validation,
        OperatorResolutionRequest {
            round_id: "live-round".to_owned(),
            message_id: "chat-43".to_owned(),
            verdict: ResolutionVerdict::Accepted,
            target_id: Some("untrusted-target".to_owned()),
            note: String::new(),
        },
    );

    assert!(result.is_err());
}
