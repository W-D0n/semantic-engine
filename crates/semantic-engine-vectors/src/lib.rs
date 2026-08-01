//! Optional, model-agnostic vector recognition for Semantic Engine.

use std::{collections::HashSet, error::Error, fmt};

use semantic_engine_core::{
    AnswerTarget, Decision, MAX_ALIASES_PER_TARGET, MAX_EXPRESSION_CHARS, MAX_IDENTIFIER_CHARS,
    MAX_TARGETS_PER_ROUND, Round, Submission, Validator,
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

pub const VECTOR_INDEX_SCHEMA_VERSION: u32 = 1;
pub const MAX_VECTOR_DIMENSIONS: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub revision: String,
    pub fingerprint_sha256: Sha256Fingerprint,
    pub dimensions: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Sha256Fingerprint(String);

impl Sha256Fingerprint {
    pub fn parse(value: impl Into<String>) -> Result<Self, VectorError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(VectorError("model fingerprint must be a SHA-256 hex digest"));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Sha256Fingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbeddingRole {
    KnownExpression,
    Statement,
}

pub trait EmbeddingProvider {
    fn descriptor(&self) -> ModelDescriptor;
    fn embed(&mut self, role: EmbeddingRole, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorPolicy {
    pub accept_threshold: f64,
    pub review_threshold: f64,
    pub ambiguity_margin: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorRecognition {
    pub round_id: String,
    pub message_id: String,
    pub participant_id: String,
    pub source_sequence: u64,
    pub decision: Decision,
    pub target_id: Option<String>,
    pub candidate_target_id: String,
    pub score: f64,
    pub runner_up_score: Option<f64>,
    pub margin: Option<f64>,
    pub evidence: VectorEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorEvidence {
    pub matched_expression: String,
    pub ambiguous: bool,
}

impl Default for VectorPolicy {
    fn default() -> Self {
        Self { accept_threshold: 0.90, review_threshold: 0.82, ambiguity_margin: 0.05 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorIndex {
    schema_version: u32,
    context_version: String,
    model: ModelDescriptor,
    entries: Vec<VectorEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct VectorEntry {
    target_id: String,
    expression: String,
    vector: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorError(&'static str);

impl fmt::Display for VectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for VectorError {}

impl VectorIndex {
    pub fn build(
        context_version: &str,
        targets: &[AnswerTarget],
        provider: &mut impl EmbeddingProvider,
    ) -> Result<Self, VectorError> {
        validate_context_version(context_version)?;
        validate_targets(targets)?;
        let model = provider.descriptor();
        validate_model(&model)?;

        let expressions = targets
            .iter()
            .flat_map(|target| {
                std::iter::once(target.canonical.as_str())
                    .chain(target.aliases.iter().map(String::as_str))
                    .map(move |expression| (target.id.as_str(), expression))
            })
            .collect::<Vec<_>>();
        let inputs =
            expressions.iter().map(|(_, expression)| (*expression).to_owned()).collect::<Vec<_>>();
        let vectors = provider
            .embed(EmbeddingRole::KnownExpression, &inputs)
            .map_err(|_| VectorError("embedding provider failed"))?;
        if vectors.len() != expressions.len() {
            return Err(VectorError("embedding provider returned an invalid vector count"));
        }

        let entries = expressions
            .into_iter()
            .zip(vectors)
            .map(|((target_id, expression), vector)| {
                validate_vector(&vector, model.dimensions)?;
                Ok(VectorEntry {
                    target_id: target_id.to_owned(),
                    expression: expression.to_owned(),
                    vector,
                })
            })
            .collect::<Result<Vec<_>, VectorError>>()?;

        let index = Self {
            schema_version: VECTOR_INDEX_SCHEMA_VERSION,
            context_version: context_version.to_owned(),
            model,
            entries,
        };
        index.validate_contents()?;
        Ok(index)
    }

    pub fn recognize(
        &self,
        context_version: &str,
        round: &Round,
        submission: &Submission,
        provider: &mut impl EmbeddingProvider,
        policy: VectorPolicy,
    ) -> Result<VectorRecognition, VectorError> {
        if self.schema_version != VECTOR_INDEX_SCHEMA_VERSION {
            return Err(VectorError("unsupported vector index schema version"));
        }
        self.validate_contents()?;
        if self.context_version != context_version {
            return Err(VectorError("vector index context version mismatch"));
        }
        if self.model != provider.descriptor() {
            return Err(VectorError("vector index model mismatch"));
        }
        validate_policy(policy)?;

        let defensive_validation = Validator::default().validate(round, submission);
        if defensive_validation.issue.is_some() {
            return Err(VectorError("round or submission is invalid"));
        }

        let target_ids =
            round.targets.iter().map(|target| target.id.as_str()).collect::<HashSet<_>>();
        if target_ids
            .iter()
            .any(|target_id| !self.entries.iter().any(|entry| entry.target_id == **target_id))
        {
            return Err(VectorError("vector index is missing a round target"));
        }

        let query = provider
            .embed(EmbeddingRole::Statement, std::slice::from_ref(&submission.text))
            .map_err(|_| VectorError("embedding provider failed"))?;
        if query.len() != 1 {
            return Err(VectorError("embedding provider returned an invalid vector count"));
        }
        validate_vector(&query[0], self.model.dimensions)?;

        let mut candidates = round
            .targets
            .iter()
            .map(|target| {
                self.entries
                    .iter()
                    .filter(|entry| entry.target_id == target.id)
                    .map(|entry| (entry, cosine_similarity(&query[0], &entry.vector)))
                    .reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best })
                    .expect("a checked target has at least one indexed expression")
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|(_, left), (_, right)| right.total_cmp(left));

        let (best_entry, best_score) = candidates[0];
        let runner_up_score = candidates.get(1).map(|(_, score)| *score);
        let margin = runner_up_score.map(|score| best_score - score);
        let ambiguous = margin.is_some_and(|margin| margin < policy.ambiguity_margin);
        let decision = if best_score >= policy.accept_threshold && !ambiguous {
            Decision::Accepted
        } else if best_score >= policy.review_threshold || ambiguous {
            Decision::Abstained
        } else {
            Decision::Rejected
        };
        let target_id = (decision == Decision::Accepted).then(|| best_entry.target_id.clone());

        Ok(VectorRecognition {
            round_id: round.id.clone(),
            message_id: submission.message_id.clone(),
            participant_id: submission.participant_id.clone(),
            source_sequence: submission.source_sequence,
            decision,
            target_id,
            candidate_target_id: best_entry.target_id.clone(),
            score: best_score,
            runner_up_score,
            margin,
            evidence: VectorEvidence {
                matched_expression: best_entry.expression.clone(),
                ambiguous,
            },
        })
    }

    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    #[must_use]
    pub fn context_version(&self) -> &str {
        &self.context_version
    }

    #[must_use]
    pub fn model(&self) -> &ModelDescriptor {
        &self.model
    }

    #[must_use]
    pub fn expression_count(&self) -> usize {
        self.entries.len()
    }

    fn validate_contents(&self) -> Result<(), VectorError> {
        validate_context_version(&self.context_version)
            .map_err(|_| VectorError("vector index contents are invalid"))?;
        validate_model(&self.model)
            .map_err(|_| VectorError("vector index contents are invalid"))?;
        if self.entries.is_empty()
            || self.entries.len() > MAX_TARGETS_PER_ROUND * (MAX_ALIASES_PER_TARGET + 1)
        {
            return Err(VectorError("vector index contents are invalid"));
        }
        let mut expressions = HashSet::with_capacity(self.entries.len());
        for entry in &self.entries {
            if entry.target_id.is_empty()
                || entry.target_id.chars().count() > MAX_IDENTIFIER_CHARS
                || entry.expression.is_empty()
                || entry.expression.chars().count() > MAX_EXPRESSION_CHARS
                || !expressions.insert((entry.target_id.as_str(), entry.expression.as_str()))
                || validate_vector(&entry.vector, self.model.dimensions).is_err()
            {
                return Err(VectorError("vector index contents are invalid"));
            }
        }
        Ok(())
    }
}

fn validate_context_version(context_version: &str) -> Result<(), VectorError> {
    if context_version.is_empty() || context_version.chars().count() > MAX_IDENTIFIER_CHARS {
        return Err(VectorError("vector index context version is invalid"));
    }
    Ok(())
}

fn validate_model(model: &ModelDescriptor) -> Result<(), VectorError> {
    let invalid_text =
        |value: &str| value.is_empty() || value.chars().count() > MAX_IDENTIFIER_CHARS;
    if invalid_text(&model.id)
        || invalid_text(&model.revision)
        || model.dimensions == 0
        || model.dimensions > MAX_VECTOR_DIMENSIONS
    {
        return Err(VectorError("embedding model descriptor is invalid"));
    }
    Ok(())
}

fn validate_targets(targets: &[AnswerTarget]) -> Result<(), VectorError> {
    if targets.is_empty() || targets.len() > MAX_TARGETS_PER_ROUND {
        return Err(VectorError("vector index targets are invalid"));
    }
    let mut ids = HashSet::with_capacity(targets.len());
    for target in targets {
        if target.id.is_empty()
            || target.id.chars().count() > MAX_IDENTIFIER_CHARS
            || !ids.insert(target.id.as_str())
            || target.canonical.is_empty()
            || target.canonical.chars().count() > MAX_EXPRESSION_CHARS
            || target.aliases.len() > MAX_ALIASES_PER_TARGET
            || target
                .aliases
                .iter()
                .any(|alias| alias.is_empty() || alias.chars().count() > MAX_EXPRESSION_CHARS)
        {
            return Err(VectorError("vector index targets are invalid"));
        }
    }
    Ok(())
}

fn validate_policy(policy: VectorPolicy) -> Result<(), VectorError> {
    if !policy.accept_threshold.is_finite()
        || !policy.review_threshold.is_finite()
        || !policy.ambiguity_margin.is_finite()
        || !(0.0..=1.0).contains(&policy.accept_threshold)
        || !(0.0..=1.0).contains(&policy.review_threshold)
        || !(0.0..=1.0).contains(&policy.ambiguity_margin)
        || policy.review_threshold > policy.accept_threshold
    {
        return Err(VectorError("vector recognition policy is invalid"));
    }
    Ok(())
}

fn validate_vector(vector: &[f32], dimensions: usize) -> Result<(), VectorError> {
    if vector.len() != dimensions
        || vector.iter().any(|value| !value.is_finite())
        || vector.iter().all(|value| *value == 0.0)
    {
        return Err(VectorError("embedding provider returned an invalid vector"));
    }
    Ok(())
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    let (dot, left_norm, right_norm) = left.iter().zip(right).fold(
        (0.0_f64, 0.0_f64, 0.0_f64),
        |(dot, left_norm, right_norm), (left, right)| {
            let left = f64::from(*left);
            let right = f64::from(*right);
            (dot + left * right, left_norm + left * left, right_norm + right * right)
        },
    );
    (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(0.0, 1.0)
}
