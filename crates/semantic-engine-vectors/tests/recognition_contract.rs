use semantic_engine_core::{AnswerTarget, Decision, Round, Submission, ValidationPolicy};
use semantic_engine_vectors::{
    EmbeddingProvider, EmbeddingRole, ModelDescriptor, Sha256Fingerprint, VectorIndex, VectorPolicy,
};

struct FixedProvider {
    descriptor: ModelDescriptor,
}

impl FixedProvider {
    fn new() -> Self {
        Self {
            descriptor: ModelDescriptor {
                id: "fixed-test-model".into(),
                revision: "fixture-v1".into(),
                fingerprint_sha256: Sha256Fingerprint::parse("11".repeat(32))
                    .expect("fixture fingerprint should be valid"),
                dimensions: 2,
            },
        }
    }
}

impl EmbeddingProvider for FixedProvider {
    fn descriptor(&self) -> ModelDescriptor {
        self.descriptor.clone()
    }

    fn embed(&mut self, _role: EmbeddingRole, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        texts
            .iter()
            .map(|text| match text.as_str() {
                "Portal" | "Portal II" | "le jeu des portails" => Ok(vec![1.0, 0.0]),
                "Hades" | "le roguelite des enfers" => Ok(vec![0.0, 1.0]),
                "aucune idee" => Ok(vec![0.5, 0.5]),
                other => Err(format!("missing fixture embedding for {other}")),
            })
            .collect()
    }
}

fn round() -> Round {
    Round {
        id: "round-1".into(),
        targets: vec![
            AnswerTarget {
                id: "portal".into(),
                canonical: "Portal".into(),
                aliases: vec!["Portal II".into()],
            },
            AnswerTarget { id: "hades".into(), canonical: "Hades".into(), aliases: vec![] },
        ],
        policy: ValidationPolicy::default(),
    }
}

fn submission(text: &str) -> Submission {
    Submission {
        message_id: format!("message-{text}"),
        participant_id: "viewer-1".into(),
        source_sequence: 1,
        text: text.into(),
    }
}

#[test]
fn versioned_index_recognizes_a_semantic_expression_with_evidence() {
    let mut provider = FixedProvider::new();
    let round = round();
    let index = VectorIndex::build("answer-atlas@1.2.0", &round.targets, &mut provider)
        .expect("fixture index should build");

    let validation = index
        .recognize(
            "answer-atlas@1.2.0",
            &round,
            &submission("le jeu des portails"),
            &mut provider,
            VectorPolicy::default(),
        )
        .expect("fixture statement should be recognized");

    assert_eq!(validation.decision, Decision::Accepted);
    assert_eq!(validation.target_id.as_deref(), Some("portal"));
    assert!(!validation.evidence.ambiguous);
    assert_eq!(validation.evidence.matched_expression, "Portal");
}

#[test]
fn index_refuses_a_different_context_or_model_revision() {
    let mut provider = FixedProvider::new();
    let round = round();
    let index = VectorIndex::build("answer-atlas@1.2.0", &round.targets, &mut provider)
        .expect("fixture index should build");

    let context_error = index
        .recognize(
            "answer-atlas@1.3.0",
            &round,
            &submission("le jeu des portails"),
            &mut provider,
            VectorPolicy::default(),
        )
        .expect_err("another context version must not reuse the index");
    assert_eq!(context_error.to_string(), "vector index context version mismatch");

    provider.descriptor.revision = "sha256:changed".into();
    let model_error = index
        .recognize(
            "answer-atlas@1.2.0",
            &round,
            &submission("le jeu des portails"),
            &mut provider,
            VectorPolicy::default(),
        )
        .expect_err("another model revision must not reuse the index");
    assert_eq!(model_error.to_string(), "vector index model mismatch");
}

#[test]
fn an_ambiguous_vector_abstains_instead_of_guessing() {
    let mut provider = FixedProvider::new();
    let round = round();
    let index = VectorIndex::build("answer-atlas@1.2.0", &round.targets, &mut provider)
        .expect("fixture index should build");

    let validation = index
        .recognize(
            "answer-atlas@1.2.0",
            &round,
            &submission("aucune idee"),
            &mut provider,
            VectorPolicy { accept_threshold: 0.6, review_threshold: 0.5, ambiguity_margin: 0.1 },
        )
        .expect("ambiguous input should produce a validation");

    assert_eq!(validation.decision, Decision::Abstained);
    assert_eq!(validation.target_id, None);
    assert!(validation.evidence.ambiguous);
}

#[test]
fn serialized_index_round_trips_without_provider_state() {
    let mut provider = FixedProvider::new();
    let index = VectorIndex::build("answer-atlas@1.2.0", &round().targets, &mut provider)
        .expect("fixture index should build");

    let json = serde_json::to_string(&index).expect("index should serialize");
    let restored: VectorIndex = serde_json::from_str(&json).expect("index should deserialize");

    assert_eq!(restored, index);
    assert_eq!(restored.schema_version(), 1);
    assert_eq!(restored.context_version(), "answer-atlas@1.2.0");
    assert_eq!(restored.model(), &provider.descriptor());
}

#[test]
fn a_tampered_serialized_vector_is_rejected_before_scoring() {
    let mut provider = FixedProvider::new();
    let index = VectorIndex::build("answer-atlas@1.2.0", &round().targets, &mut provider)
        .expect("fixture index should build");
    let mut value = serde_json::to_value(index).expect("index should serialize");
    value["entries"][0]["vector"] = serde_json::json!([0.0, 0.0]);
    let tampered: VectorIndex = serde_json::from_value(value).expect("shape remains valid JSON");

    let error = tampered
        .recognize(
            "answer-atlas@1.2.0",
            &round(),
            &submission("le jeu des portails"),
            &mut provider,
            VectorPolicy::default(),
        )
        .expect_err("a zero vector must not reach cosine scoring");

    assert_eq!(error.to_string(), "vector index contents are invalid");
}

#[test]
fn duplicate_expressions_are_rejected_while_building_the_index() {
    let mut provider = FixedProvider::new();
    let mut target = round().targets.remove(0);
    target.aliases.push("Portal".into());

    let error = VectorIndex::build("answer-atlas@1.2.0", &[target], &mut provider)
        .expect_err("a duplicate expression must not produce a reusable index");

    assert_eq!(error.to_string(), "vector index contents are invalid");
}
