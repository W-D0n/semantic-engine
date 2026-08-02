use serde::{Deserialize, Serialize};
use strsim::{jaro_winkler, normalized_damerau_levenshtein};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub const MAX_SUBMISSION_CHARS: usize = 1_000;
pub const MAX_IDENTIFIER_CHARS: usize = 256;
pub const MAX_EXPRESSION_CHARS: usize = 256;
pub const MAX_ALIASES_PER_TARGET: usize = 64;
pub const MAX_TARGETS_PER_ROUND: usize = 256;
pub const MAX_RESOLUTION_NOTE_CHARS: usize = 512;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerTarget {
    pub id: String,
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Round {
    pub id: String,
    pub targets: Vec<AnswerTarget>,
    #[serde(default)]
    pub policy: ValidationPolicy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Submission {
    pub message_id: String,
    pub participant_id: String,
    pub source_sequence: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidationPolicy {
    pub accept_threshold: f64,
    pub review_threshold: f64,
    #[serde(default = "default_ambiguity_margin")]
    pub ambiguity_margin: f64,
}

impl Default for ValidationPolicy {
    fn default() -> Self {
        Self {
            accept_threshold: 0.87,
            review_threshold: 0.72,
            ambiguity_margin: default_ambiguity_margin(),
        }
    }
}

fn default_ambiguity_margin() -> f64 {
    0.05
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accepted,
    Rejected,
    Abstained,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ConfiguredExpression,
    NormalizedExpression,
    MemoryExpression,
    FuzzyExpression,
    AmbiguousExpression,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationIssue {
    InvalidPolicy,
    InvalidRound,
    InvalidSubmission,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub matched_expression: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Validation {
    pub round_id: String,
    pub message_id: String,
    pub participant_id: String,
    pub source_sequence: u64,
    pub decision: Decision,
    pub target_id: Option<String>,
    pub score: f64,
    pub evidence: Vec<Evidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<ValidationIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionVerdict {
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorResolutionRequest {
    pub round_id: String,
    pub message_id: String,
    pub verdict: ResolutionVerdict,
    pub target_id: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperatorResolution {
    pub round_id: String,
    pub message_id: String,
    pub participant_id: String,
    pub source_sequence: u64,
    pub original_decision: Decision,
    pub final_decision: Decision,
    pub target_id: Option<String>,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionIssue {
    ValidationMismatch,
    InvalidTarget,
    InvalidNote,
}

pub fn resolve_validation(
    round: &Round,
    validation: &Validation,
    request: OperatorResolutionRequest,
) -> Result<OperatorResolution, ResolutionIssue> {
    if validation.round_id != round.id
        || request.round_id != round.id
        || validation.message_id != request.message_id
        || request.message_id.is_empty()
    {
        return Err(ResolutionIssue::ValidationMismatch);
    }
    if request.note.chars().count() > MAX_RESOLUTION_NOTE_CHARS {
        return Err(ResolutionIssue::InvalidNote);
    }

    let (final_decision, target_id) = match request.verdict {
        ResolutionVerdict::Accepted => {
            let target_id = request.target_id.ok_or(ResolutionIssue::InvalidTarget)?;
            if !round.targets.iter().any(|target| target.id == target_id) {
                return Err(ResolutionIssue::InvalidTarget);
            }
            (Decision::Accepted, Some(target_id))
        }
        ResolutionVerdict::Rejected => {
            if request.target_id.is_some() {
                return Err(ResolutionIssue::InvalidTarget);
            }
            (Decision::Rejected, None)
        }
    };

    Ok(OperatorResolution {
        round_id: validation.round_id.clone(),
        message_id: validation.message_id.clone(),
        participant_id: validation.participant_id.clone(),
        source_sequence: validation.source_sequence,
        original_decision: validation.decision.clone(),
        final_decision,
        target_id,
        note: request.note,
    })
}
#[derive(Clone, Debug, Default)]
pub struct Validator {
    _private: (),
}

impl Validator {
    #[must_use]
    pub fn validate(&self, round: &Round, submission: &Submission) -> Validation {
        if let Some(issue) = validation_issue(round, submission) {
            return invalid(round, submission, issue);
        }

        let normalized_submission = normalize(&submission.text);
        let compact_submission = compact(&normalized_submission);

        let exact_matches: Vec<_> = round
            .targets
            .iter()
            .filter_map(|target| {
                std::iter::once(&target.canonical)
                    .chain(target.aliases.iter())
                    .find(|expression| {
                        let normalized_expression = normalize(expression);
                        !normalized_submission.is_empty()
                            && (normalized_expression == normalized_submission
                                || compact(&normalized_expression) == compact_submission)
                    })
                    .map(|expression| (target, expression))
            })
            .collect();

        match exact_matches.as_slice() {
            [] => self.validate_fuzzy(round, submission, &compact_submission),
            [(target, expression)] => Validation {
                round_id: round.id.clone(),
                message_id: submission.message_id.clone(),
                participant_id: submission.participant_id.clone(),
                source_sequence: submission.source_sequence,
                decision: Decision::Accepted,
                target_id: Some(target.id.clone()),
                score: 1.0,
                evidence: vec![Evidence {
                    kind: if expression.as_str() == submission.text {
                        EvidenceKind::ConfiguredExpression
                    } else {
                        EvidenceKind::NormalizedExpression
                    },
                    matched_expression: (*expression).clone(),
                }],
                issue: None,
            },
            _ => Validation {
                round_id: round.id.clone(),
                message_id: submission.message_id.clone(),
                participant_id: submission.participant_id.clone(),
                source_sequence: submission.source_sequence,
                decision: Decision::Abstained,
                target_id: None,
                score: 1.0,
                evidence: vec![Evidence {
                    kind: EvidenceKind::AmbiguousExpression,
                    matched_expression: submission.text.clone(),
                }],
                issue: None,
            },
        }
    }

    fn validate_fuzzy(
        &self,
        round: &Round,
        submission: &Submission,
        compact_submission: &str,
    ) -> Validation {
        if compact_submission.chars().count() < 4 {
            return rejected(round, submission, 0.0);
        }

        let mut candidates: Vec<_> = round
            .targets
            .iter()
            .filter_map(|target| {
                std::iter::once(&target.canonical)
                    .chain(target.aliases.iter())
                    .map(|expression| {
                        let candidate = compact(&normalize(expression));
                        let score = fuzzy_score(compact_submission, &candidate);
                        (target, expression, score)
                    })
                    .max_by(|left, right| left.2.total_cmp(&right.2))
            })
            .collect();
        candidates.sort_by(|left, right| right.2.total_cmp(&left.2));

        let Some((target, expression, score)) = candidates.first() else {
            return rejected(round, submission, 0.0);
        };
        let ambiguous = candidates.get(1).is_some_and(|runner_up| {
            runner_up.2 >= round.policy.review_threshold
                && score - runner_up.2 < round.policy.ambiguity_margin
        });

        let decision = if *score >= round.policy.accept_threshold && !ambiguous {
            Decision::Accepted
        } else if *score >= round.policy.review_threshold {
            Decision::Abstained
        } else {
            Decision::Rejected
        };

        if decision == Decision::Rejected {
            rejected(round, submission, *score)
        } else {
            Validation {
                round_id: round.id.clone(),
                message_id: submission.message_id.clone(),
                participant_id: submission.participant_id.clone(),
                source_sequence: submission.source_sequence,
                decision,
                target_id: (!ambiguous).then(|| target.id.clone()),
                score: *score,
                evidence: vec![Evidence {
                    kind: if ambiguous {
                        EvidenceKind::AmbiguousExpression
                    } else {
                        EvidenceKind::FuzzyExpression
                    },
                    matched_expression: (*expression).clone(),
                }],
                issue: None,
            }
        }
    }
}

fn rejected(round: &Round, submission: &Submission, score: f64) -> Validation {
    Validation {
        round_id: round.id.clone(),
        message_id: submission.message_id.clone(),
        participant_id: submission.participant_id.clone(),
        source_sequence: submission.source_sequence,
        decision: Decision::Rejected,
        target_id: None,
        score,
        evidence: Vec::new(),
        issue: None,
    }
}

fn invalid(round: &Round, submission: &Submission, issue: ValidationIssue) -> Validation {
    let mut validation = rejected(round, submission, 0.0);
    validation.issue = Some(issue);
    validation
}

#[must_use]
pub fn normalize_expression(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut pending_space = false;

    for character in
        input.nfkd().filter(|character| !is_combining_mark(*character)).flat_map(char::to_lowercase)
    {
        if character.is_alphanumeric() {
            if pending_space && !output.is_empty() {
                output.push(' ');
            }
            pending_space = false;
            output.push(character);
        } else {
            pending_space = true;
        }
    }

    output
}

fn normalize(input: &str) -> String {
    normalize_expression(input)
}

fn compact(normalized: &str) -> String {
    normalized.chars().filter(|character| !character.is_whitespace()).collect()
}

fn fuzzy_score(submission: &str, candidate: &str) -> f64 {
    let submission_digits: String = submission.chars().filter(char::is_ascii_digit).collect();
    let candidate_digits: String = candidate.chars().filter(char::is_ascii_digit).collect();

    if submission_digits != candidate_digits {
        return 0.0;
    }

    let edit_score = normalized_damerau_levenshtein(submission, candidate);
    if submission.chars().count() == candidate.chars().count() {
        edit_score.max(jaro_winkler(submission, candidate))
    } else {
        edit_score
    }
}

fn validation_issue(round: &Round, submission: &Submission) -> Option<ValidationIssue> {
    let valid_policy = (0.0..=1.0).contains(&round.policy.review_threshold)
        && (0.0..=1.0).contains(&round.policy.accept_threshold)
        && round.policy.accept_threshold >= round.policy.review_threshold
        && (0.0..=1.0).contains(&round.policy.ambiguity_margin);
    if !valid_policy {
        return Some(ValidationIssue::InvalidPolicy);
    }

    let valid_submission = !submission.message_id.is_empty()
        && submission.message_id.chars().count() <= MAX_IDENTIFIER_CHARS
        && !submission.participant_id.is_empty()
        && submission.participant_id.chars().count() <= MAX_IDENTIFIER_CHARS
        && submission.text.chars().count() <= MAX_SUBMISSION_CHARS;
    if !valid_submission {
        return Some(ValidationIssue::InvalidSubmission);
    }

    let valid_round = !round.id.is_empty()
        && round.id.chars().count() <= MAX_IDENTIFIER_CHARS
        && !round.targets.is_empty()
        && round.targets.len() <= MAX_TARGETS_PER_ROUND;
    let valid_targets = round.targets.iter().all(|target| {
        !target.id.is_empty()
            && target.id.chars().count() <= MAX_IDENTIFIER_CHARS
            && !target.canonical.is_empty()
            && target.canonical.chars().count() <= MAX_EXPRESSION_CHARS
            && target.aliases.len() <= MAX_ALIASES_PER_TARGET
            && target
                .aliases
                .iter()
                .all(|alias| !alias.is_empty() && alias.chars().count() <= MAX_EXPRESSION_CHARS)
    });

    if !valid_round || !valid_targets {
        return Some(ValidationIssue::InvalidRound);
    }

    None
}
