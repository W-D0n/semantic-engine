use std::{fs, path::PathBuf};

use semantic_engine_core::{
    AnswerTarget, Decision, Round, Submission, ValidationPolicy, Validator,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct TitleCorpus {
    titles: Vec<Title>,
}

#[derive(Deserialize)]
struct Title {
    id: String,
    canonical: String,
    aliases: Vec<String>,
}

#[derive(Deserialize)]
struct CaseCorpus {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    target_id: String,
    input: String,
    expected: String,
}

fn corpus_path(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus").join(file)
}

#[test]
fn curated_movie_and_game_cases_follow_the_validation_contract() {
    let titles: TitleCorpus = serde_json::from_str(
        &fs::read_to_string(corpus_path("titles.json")).expect("title corpus must be readable"),
    )
    .expect("title corpus must be valid JSON");
    let cases: CaseCorpus = serde_json::from_str(
        &fs::read_to_string(corpus_path("cases.json")).expect("case corpus must be readable"),
    )
    .expect("case corpus must be valid JSON");

    for (sequence, case) in cases.cases.iter().enumerate() {
        let title = titles
            .titles
            .iter()
            .find(|title| title.id == case.target_id)
            .unwrap_or_else(|| panic!("missing title {}", case.target_id));
        let round = Round {
            id: format!("corpus-{}", title.id),
            targets: vec![AnswerTarget {
                id: title.id.clone(),
                canonical: title.canonical.clone(),
                aliases: title.aliases.clone(),
            }],
            policy: ValidationPolicy::default(),
        };
        let validation = Validator::default().validate(
            &round,
            &Submission {
                message_id: format!("corpus-{sequence}"),
                participant_id: "corpus-runner".into(),
                source_sequence: sequence as u64,
                text: case.input.clone(),
            },
        );
        let expected = match case.expected.as_str() {
            "accepted" => Decision::Accepted,
            "abstained" => Decision::Abstained,
            "rejected" => Decision::Rejected,
            other => panic!("unknown expected decision {other}"),
        };

        assert_eq!(
            validation.decision, expected,
            "target={}, input={:?}, score={}",
            title.id, case.input, validation.score
        );
    }
}
