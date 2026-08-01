use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use semantic_engine_core::{
    AnswerTarget, Decision, Round, Submission, ValidationPolicy, Validator,
};
use semantic_engine_vectors::{
    EmbeddingProvider, EmbeddingRole, ModelDescriptor, Sha256Fingerprint, VectorIndex, VectorPolicy,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MODEL_ID: &str = "intfloat/multilingual-e5-small";
const MODEL_REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";
const MODEL_DIMENSIONS: usize = 384;

#[derive(Deserialize)]
struct TitleCorpus {
    version: u32,
    titles: Vec<Title>,
}

#[derive(Clone, Deserialize)]
struct Title {
    id: String,
    canonical: String,
    aliases: Vec<String>,
}

impl Title {
    fn as_target(&self) -> AnswerTarget {
        AnswerTarget {
            id: self.id.clone(),
            canonical: self.canonical.clone(),
            aliases: self.aliases.clone(),
        }
    }
}

#[derive(Deserialize)]
struct CaseCorpus {
    version: u32,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct MultiTargetCorpus {
    version: u32,
    cases: Vec<MultiTargetCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    target_id: String,
    #[serde(rename = "input")]
    statement: String,
    expected: String,
    category: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MultiTargetCase {
    #[serde(rename = "input")]
    statement: String,
    expected: String,
    target_id: Option<String>,
    category: String,
}

struct FastEmbedProvider {
    model: TextEmbedding,
    descriptor: ModelDescriptor,
}

impl FastEmbedProvider {
    fn load(cache_dir: &Path) -> Result<Self, Box<dyn Error>> {
        fs::create_dir_all(cache_dir)?;
        let model = TextEmbedding::try_new(
            TextInitOptions::new(EmbeddingModel::MultilingualE5Small)
                .with_cache_dir(cache_dir.to_path_buf())
                .with_show_download_progress(true),
        )?;
        let fingerprint_sha256 = Sha256Fingerprint::parse(fingerprint_tree(cache_dir)?)?;
        Ok(Self {
            model,
            descriptor: ModelDescriptor {
                id: MODEL_ID.into(),
                revision: MODEL_REVISION.into(),
                fingerprint_sha256,
                dimensions: MODEL_DIMENSIONS,
            },
        })
    }
}

impl EmbeddingProvider for FastEmbedProvider {
    fn descriptor(&self) -> ModelDescriptor {
        self.descriptor.clone()
    }

    fn embed(&mut self, role: EmbeddingRole, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let inputs = texts
            .iter()
            .map(|text| match role {
                EmbeddingRole::KnownExpression | EmbeddingRole::Statement => {
                    format!("query: {text}")
                }
            })
            .collect::<Vec<_>>();
        self.model.embed(inputs, None).map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
struct Sample {
    expected: Decision,
    lexical: Decision,
    vector_score: f64,
    category: String,
}

struct RetrievalSample {
    expected: Decision,
    expected_target_id: Option<String>,
    lexical: Decision,
    lexical_target_id: Option<String>,
    candidate_target_id: String,
    score: f64,
    runner_up_score: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
struct Metrics {
    accepted_precision: f64,
    accepted_recall: f64,
    decision_accuracy: f64,
    false_accepts: u64,
    confusion: BTreeMap<String, u64>,
    categories: BTreeMap<String, CategoryMetrics>,
}

#[derive(Clone, Debug, Default, Serialize)]
struct CategoryMetrics {
    annotations: u64,
    correct: u64,
    accuracy: f64,
}

#[derive(Serialize)]
struct LatencySummary {
    samples: usize,
    p50_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
    maximum_ns: u64,
}

#[derive(Serialize)]
struct Report {
    report_version: u32,
    generated_at_unix_seconds: u64,
    title_corpus_version: u32,
    case_corpus_version: u32,
    multi_target_corpus_version: u32,
    titles: usize,
    annotations: usize,
    multi_target_annotations: usize,
    model: ModelDescriptor,
    model_cache_bytes: u64,
    model_initialization_ms: u128,
    vector_index: IndexReport,
    lexical: EngineReport,
    vector_default: VectorReport,
    vector_calibrated: VectorReport,
    recommendation: &'static str,
    rationale: Vec<&'static str>,
}

#[derive(Serialize)]
struct IndexReport {
    context_version: String,
    schema_version: u32,
    expressions: usize,
    serialized_bytes: usize,
    build_ms: u128,
    path: String,
}

#[derive(Serialize)]
struct EngineReport {
    metrics: Metrics,
    latency: LatencySummary,
    multi_target: RetrievalMetrics,
    multi_target_latency: LatencySummary,
}

#[derive(Serialize)]
struct VectorReport {
    policy: VectorPolicy,
    metrics: Metrics,
    multi_target: RetrievalMetrics,
    query_latency: LatencySummary,
    multi_target_query_latency: LatencySummary,
    quality_gate_passed: bool,
}

#[derive(Clone, Debug, Serialize)]
struct RetrievalMetrics {
    annotations: u64,
    positive_annotations: u64,
    top1_correct: u64,
    top1_accuracy: f64,
    accepted_precision: f64,
    accepted_recall: f64,
    decision_accuracy: f64,
    false_accepts: u64,
    accepted_correct: u64,
    accepted_wrong: u64,
    abstained: u64,
    rejected: u64,
    confusion: BTreeMap<String, u64>,
}

struct Options {
    titles: PathBuf,
    cases: PathBuf,
    multi_target_cases: PathBuf,
    cache_dir: PathBuf,
    output: PathBuf,
    index_output: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_options(env::args().skip(1))?;
    let titles: TitleCorpus = serde_json::from_str(&fs::read_to_string(&options.titles)?)?;
    let cases: CaseCorpus = serde_json::from_str(&fs::read_to_string(&options.cases)?)?;
    let multi_target_cases: MultiTargetCorpus =
        serde_json::from_str(&fs::read_to_string(&options.multi_target_cases)?)?;
    validate_corpus(&titles, &cases, &multi_target_cases)?;

    let model_started = Instant::now();
    let mut provider = FastEmbedProvider::load(&options.cache_dir)?;
    let model_initialization_ms = model_started.elapsed().as_millis();

    let targets = titles.titles.iter().map(Title::as_target).collect::<Vec<_>>();
    let context_version = format!("title-corpus-v{}", titles.version);
    let index_started = Instant::now();
    let index = VectorIndex::build(&context_version, &targets, &mut provider)?;
    let index_build_ms = index_started.elapsed().as_millis();
    let index_json = serde_json::to_vec(&index)?;
    write_replaceable_file(&options.index_output, &index_json)?;

    let validator = Validator::default();
    let mut samples = Vec::with_capacity(cases.cases.len());
    let mut lexical_latencies = Vec::with_capacity(cases.cases.len());
    let mut vector_latencies = Vec::with_capacity(cases.cases.len());
    let mut multi_target_lexical_latencies = Vec::with_capacity(multi_target_cases.cases.len());
    let mut multi_target_vector_latencies = Vec::with_capacity(multi_target_cases.cases.len());
    let mut retrieval_samples = Vec::with_capacity(multi_target_cases.cases.len());
    let all_targets_round = Round {
        id: "vector-benchmark-all-targets".into(),
        targets: targets.clone(),
        policy: ValidationPolicy::default(),
    };
    let score_policy =
        VectorPolicy { accept_threshold: 0.0, review_threshold: 0.0, ambiguity_margin: 0.0 };

    for (sequence, case) in cases.cases.iter().enumerate() {
        let title = titles
            .titles
            .iter()
            .find(|title| title.id == case.target_id)
            .ok_or("case refers to an unknown title")?;
        let round = Round {
            id: format!("vector-benchmark-{}", title.id),
            targets: vec![title.as_target()],
            policy: ValidationPolicy::default(),
        };
        let submission = Submission {
            message_id: format!("vector-benchmark-{sequence}"),
            participant_id: "benchmark-runner".into(),
            source_sequence: u64::try_from(sequence)?,
            text: case.statement.clone(),
        };

        let lexical_started = Instant::now();
        let lexical = validator.validate(&round, &submission);
        lexical_latencies.push(elapsed_ns(lexical_started));

        let vector_started = Instant::now();
        let vector =
            index.recognize(&context_version, &round, &submission, &mut provider, score_policy)?;
        vector_latencies.push(elapsed_ns(vector_started));

        let expected = parse_decision(&case.expected)?;
        samples.push(Sample {
            expected,
            lexical: lexical.decision,
            vector_score: vector.score,
            category: case.category.clone(),
        });
    }

    for (sequence, case) in multi_target_cases.cases.iter().enumerate() {
        let submission = Submission {
            message_id: format!("vector-benchmark-global-{sequence}"),
            participant_id: "benchmark-runner".into(),
            source_sequence: u64::try_from(sequence)?,
            text: case.statement.clone(),
        };
        let lexical_started = Instant::now();
        let lexical = validator.validate(&all_targets_round, &submission);
        multi_target_lexical_latencies.push(elapsed_ns(lexical_started));

        let vector_started = Instant::now();
        let vector = index.recognize(
            &context_version,
            &all_targets_round,
            &submission,
            &mut provider,
            score_policy,
        )?;
        multi_target_vector_latencies.push(elapsed_ns(vector_started));
        retrieval_samples.push(RetrievalSample {
            expected: parse_decision(&case.expected)?,
            expected_target_id: case.target_id.clone(),
            lexical: lexical.decision,
            lexical_target_id: lexical.target_id,
            candidate_target_id: vector.candidate_target_id,
            score: vector.score,
            runner_up_score: vector.runner_up_score,
        });
    }

    let default_calibration = VectorPolicy::default();
    let calibrated = calibrate(&samples, &retrieval_samples);
    let lexical_metrics = evaluate(&samples, |sample| sample.lexical.clone());
    let default_metrics = evaluate(&samples, |sample| {
        vector_decision(sample.vector_score, None, default_calibration)
    });
    let calibrated_metrics =
        evaluate(&samples, |sample| vector_decision(sample.vector_score, None, calibrated));
    let default_retrieval = evaluate_retrieval(&retrieval_samples, default_calibration);
    let calibrated_retrieval = evaluate_retrieval(&retrieval_samples, calibrated);
    let lexical_retrieval = evaluate_lexical_retrieval(&retrieval_samples);
    let lexical_latency = latency_summary(&mut lexical_latencies);
    let vector_latency = latency_summary(&mut vector_latencies);
    let multi_target_lexical_latency = latency_summary(&mut multi_target_lexical_latencies);
    let multi_target_vector_latency = latency_summary(&mut multi_target_vector_latencies);

    let vector_improves_quality = calibrated_metrics.decision_accuracy
        > lexical_metrics.decision_accuracy
        && calibrated_metrics.accepted_precision >= lexical_metrics.accepted_precision
        && calibrated_metrics.accepted_recall >= lexical_metrics.accepted_recall
        && retrieval_quality_gate(&calibrated_retrieval)
        && calibrated_retrieval.accepted_precision >= lexical_retrieval.accepted_precision
        && calibrated_retrieval.accepted_recall >= lexical_retrieval.accepted_recall
        && calibrated_retrieval.decision_accuracy >= lexical_retrieval.decision_accuracy;
    let recommendation =
        if vector_improves_quality { "candidate_optional" } else { "keep_lexical_default" };
    let rationale = if vector_improves_quality {
        vec![
            "calibrated vectors improve decision quality without lowering acceptance precision or recall",
            "keep the model optional until portable size and cold-start budgets are accepted",
        ]
    } else {
        vec![
            "calibrated vectors do not beat the lexical engine on the versioned corpus",
            "the model adds cold-start, disk and per-message inference costs",
            "retain vectors behind the model-agnostic seam for future paraphrase corpora",
        ]
    };

    let report = Report {
        report_version: 2,
        generated_at_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        title_corpus_version: titles.version,
        case_corpus_version: cases.version,
        multi_target_corpus_version: multi_target_cases.version,
        titles: titles.titles.len(),
        annotations: cases.cases.len(),
        multi_target_annotations: multi_target_cases.cases.len(),
        model: provider.descriptor(),
        model_cache_bytes: tree_size(&options.cache_dir)?,
        model_initialization_ms,
        vector_index: IndexReport {
            context_version,
            schema_version: index.schema_version(),
            expressions: index.expression_count(),
            serialized_bytes: index_json.len(),
            build_ms: index_build_ms,
            path: options.index_output.display().to_string(),
        },
        lexical: EngineReport {
            metrics: lexical_metrics.clone(),
            latency: lexical_latency,
            multi_target: lexical_retrieval,
            multi_target_latency: multi_target_lexical_latency,
        },
        vector_default: VectorReport {
            policy: default_calibration,
            metrics: default_metrics.clone(),
            quality_gate_passed: quality_gate(&default_metrics)
                && retrieval_quality_gate(&default_retrieval),
            multi_target: default_retrieval,
            query_latency: LatencySummary { ..vector_latency },
            multi_target_query_latency: LatencySummary { ..multi_target_vector_latency },
        },
        vector_calibrated: VectorReport {
            policy: calibrated,
            metrics: calibrated_metrics.clone(),
            quality_gate_passed: quality_gate(&calibrated_metrics)
                && retrieval_quality_gate(&calibrated_retrieval),
            multi_target: calibrated_retrieval,
            query_latency: vector_latency,
            multi_target_query_latency: multi_target_vector_latency,
        },
        recommendation,
        rationale,
    };
    let report_json = serde_json::to_vec_pretty(&report)?;
    write_replaceable_file(&options.output, &report_json)?;
    println!("{}", String::from_utf8(report_json)?);
    Ok(())
}

fn parse_options(mut args: impl Iterator<Item = String>) -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        titles: PathBuf::from("tests/corpus/titles.json"),
        cases: PathBuf::from("tests/corpus/cases.json"),
        multi_target_cases: PathBuf::from(
            "benchmarks/vector-comparison/corpus/multi-target-cases.json",
        ),
        cache_dir: PathBuf::from("artifacts/vector-benchmark/model-cache"),
        output: PathBuf::from("artifacts/vector-benchmark/report.json"),
        index_output: PathBuf::from("artifacts/vector-benchmark/index.json"),
    };
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("every option requires a path")?;
        match flag.as_str() {
            "--titles" => options.titles = value.into(),
            "--cases" => options.cases = value.into(),
            "--multi-target-cases" => options.multi_target_cases = value.into(),
            "--cache-dir" => options.cache_dir = value.into(),
            "--output" => options.output = value.into(),
            "--index-output" => options.index_output = value.into(),
            _ => return Err("unknown vector benchmark option".into()),
        }
    }
    Ok(options)
}

fn validate_corpus(
    titles: &TitleCorpus,
    cases: &CaseCorpus,
    multi_target: &MultiTargetCorpus,
) -> Result<(), Box<dyn Error>> {
    if titles.titles.is_empty() || titles.titles.len() > 256 {
        return Err("title corpus must contain between 1 and 256 titles".into());
    }
    if cases.cases.is_empty() || cases.cases.len() > 10_000 {
        return Err("case corpus must contain between 1 and 10000 annotations".into());
    }
    if cases.cases.iter().any(|case| case.category.is_empty()) {
        return Err("case categories must not be empty".into());
    }
    if multi_target.cases.is_empty() || multi_target.cases.len() > 10_000 {
        return Err("multi-target corpus must contain between 1 and 10000 annotations".into());
    }
    for case in &multi_target.cases {
        let expected = parse_decision(&case.expected)?;
        let accepted = expected == Decision::Accepted;
        if case.category.is_empty()
            || accepted != case.target_id.is_some()
            || case
                .target_id
                .as_ref()
                .is_some_and(|target_id| !titles.titles.iter().any(|title| title.id == *target_id))
        {
            return Err("multi-target corpus contains an invalid annotation".into());
        }
    }
    Ok(())
}

fn calibrate(samples: &[Sample], retrieval_samples: &[RetrievalSample]) -> VectorPolicy {
    let mut best =
        VectorPolicy { accept_threshold: 1.0, review_threshold: 1.0, ambiguity_margin: 0.05 };
    let mut best_metrics =
        evaluate(samples, |sample| vector_decision(sample.vector_score, None, best));
    let mut best_retrieval = evaluate_retrieval(retrieval_samples, best);
    for accept_step in 70..=100 {
        let accept = f64::from(accept_step) / 100.0;
        for review_step in 50..=accept_step {
            for margin_step in 0..=10 {
                let candidate = VectorPolicy {
                    accept_threshold: accept,
                    review_threshold: f64::from(review_step) / 100.0,
                    ambiguity_margin: f64::from(margin_step) / 100.0,
                };
                let metrics = evaluate(samples, |sample| {
                    vector_decision(sample.vector_score, None, candidate)
                });
                let retrieval = evaluate_retrieval(retrieval_samples, candidate);
                if metrics_key(&metrics, &retrieval) > metrics_key(&best_metrics, &best_retrieval) {
                    best = candidate;
                    best_metrics = metrics;
                    best_retrieval = retrieval;
                }
            }
        }
    }
    best
}

fn metrics_key(
    metrics: &Metrics,
    retrieval: &RetrievalMetrics,
) -> (bool, u64, u64, u64, u64, u64, u64) {
    (
        quality_gate(metrics) && retrieval_quality_gate(retrieval),
        u64::MAX - retrieval.accepted_wrong,
        scaled(retrieval.decision_accuracy),
        retrieval.accepted_correct,
        scaled(metrics.decision_accuracy),
        scaled(metrics.accepted_precision),
        scaled(metrics.accepted_recall),
    )
}

fn scaled(value: f64) -> u64 {
    (value * 1_000_000_000.0).round() as u64
}

fn quality_gate(metrics: &Metrics) -> bool {
    metrics.accepted_precision >= 0.95 && metrics.accepted_recall >= 0.90
}

fn retrieval_quality_gate(metrics: &RetrievalMetrics) -> bool {
    metrics.accepted_precision >= 0.95 && metrics.accepted_recall >= 0.90
}

fn vector_decision(score: f64, runner_up_score: Option<f64>, policy: VectorPolicy) -> Decision {
    let ambiguous =
        runner_up_score.is_some_and(|runner_up| score - runner_up < policy.ambiguity_margin);
    if score >= policy.accept_threshold && !ambiguous {
        Decision::Accepted
    } else if score >= policy.review_threshold || ambiguous {
        Decision::Abstained
    } else {
        Decision::Rejected
    }
}

fn evaluate_retrieval(samples: &[RetrievalSample], policy: VectorPolicy) -> RetrievalMetrics {
    evaluate_multi_target(samples, |sample| {
        (
            vector_decision(sample.score, sample.runner_up_score, policy),
            Some(sample.candidate_target_id.as_str()),
        )
    })
}

fn evaluate_lexical_retrieval(samples: &[RetrievalSample]) -> RetrievalMetrics {
    evaluate_multi_target(samples, |sample| {
        (sample.lexical.clone(), sample.lexical_target_id.as_deref())
    })
}

fn evaluate_multi_target<'a>(
    samples: &'a [RetrievalSample],
    decide: impl Fn(&'a RetrievalSample) -> (Decision, Option<&'a str>),
) -> RetrievalMetrics {
    let mut metrics = RetrievalMetrics {
        annotations: u64::try_from(samples.len()).unwrap_or(u64::MAX),
        positive_annotations: 0,
        top1_correct: 0,
        top1_accuracy: 0.0,
        accepted_precision: 0.0,
        accepted_recall: 0.0,
        decision_accuracy: 0.0,
        false_accepts: 0,
        accepted_correct: 0,
        accepted_wrong: 0,
        abstained: 0,
        rejected: 0,
        confusion: BTreeMap::new(),
    };
    let mut correct = 0_u64;
    for sample in samples {
        let expected_positive = sample.expected == Decision::Accepted;
        let (actual, candidate_target_id) = decide(sample);
        let candidate_is_correct = sample.expected_target_id.as_deref() == candidate_target_id;
        metrics.positive_annotations += u64::from(expected_positive);
        metrics.top1_correct += u64::from(expected_positive && candidate_is_correct);
        let accepted_correct =
            actual == Decision::Accepted && expected_positive && candidate_is_correct;
        let case_correct =
            actual == sample.expected && (actual != Decision::Accepted || candidate_is_correct);
        correct += u64::from(case_correct);
        *metrics
            .confusion
            .entry(format!(
                "{}->{}",
                decision_name(&sample.expected),
                if actual == Decision::Accepted && expected_positive && !candidate_is_correct {
                    "accepted_wrong_target"
                } else {
                    decision_name(&actual)
                }
            ))
            .or_default() += 1;
        match actual {
            Decision::Accepted if accepted_correct => metrics.accepted_correct += 1,
            Decision::Accepted => metrics.accepted_wrong += 1,
            Decision::Abstained => metrics.abstained += 1,
            Decision::Rejected => metrics.rejected += 1,
        }
    }
    metrics.top1_accuracy = ratio(metrics.top1_correct, metrics.positive_annotations);
    metrics.accepted_precision =
        ratio(metrics.accepted_correct, metrics.accepted_correct + metrics.accepted_wrong);
    metrics.accepted_recall = ratio(metrics.accepted_correct, metrics.positive_annotations);
    metrics.decision_accuracy = ratio(correct, metrics.annotations);
    metrics.false_accepts = metrics.accepted_wrong;
    metrics
}

fn evaluate(samples: &[Sample], decide: impl Fn(&Sample) -> Decision) -> Metrics {
    let mut correct = 0_u64;
    let mut actual_accepts = 0_u64;
    let mut expected_accepts = 0_u64;
    let mut true_accepts = 0_u64;
    let mut false_accepts = 0_u64;
    let mut confusion = BTreeMap::new();
    let mut categories = BTreeMap::<String, CategoryMetrics>::new();
    for sample in samples {
        let actual = decide(sample);
        let case_correct = actual == sample.expected;
        correct += u64::from(case_correct);
        actual_accepts += u64::from(actual == Decision::Accepted);
        expected_accepts += u64::from(sample.expected == Decision::Accepted);
        true_accepts +=
            u64::from(actual == Decision::Accepted && sample.expected == Decision::Accepted);
        false_accepts +=
            u64::from(actual == Decision::Accepted && sample.expected != Decision::Accepted);
        *confusion
            .entry(format!("{}->{}", decision_name(&sample.expected), decision_name(&actual)))
            .or_default() += 1;
        let category = categories.entry(sample.category.clone()).or_default();
        category.annotations += 1;
        category.correct += u64::from(case_correct);
    }
    for category in categories.values_mut() {
        category.accuracy = ratio(category.correct, category.annotations);
    }
    Metrics {
        accepted_precision: ratio(true_accepts, actual_accepts),
        accepted_recall: ratio(true_accepts, expected_accepts),
        decision_accuracy: ratio(correct, u64::try_from(samples.len()).unwrap_or(u64::MAX)),
        false_accepts,
        confusion,
        categories,
    }
}

fn parse_decision(value: &str) -> Result<Decision, Box<dyn Error>> {
    match value {
        "accepted" => Ok(Decision::Accepted),
        "abstained" => Ok(Decision::Abstained),
        "rejected" => Ok(Decision::Rejected),
        _ => Err("unknown expected decision".into()),
    }
}

fn decision_name(value: &Decision) -> &'static str {
    match value {
        Decision::Accepted => "accepted",
        Decision::Abstained => "abstained",
        Decision::Rejected => "rejected",
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 { 1.0 } else { numerator as f64 / denominator as f64 }
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn latency_summary(samples: &mut [u64]) -> LatencySummary {
    samples.sort_unstable();
    LatencySummary {
        samples: samples.len(),
        p50_ns: percentile(samples, 50),
        p95_ns: percentile(samples, 95),
        p99_ns: percentile(samples, 99),
        maximum_ns: samples.last().copied().unwrap_or(0),
    }
}

fn percentile(samples: &[u64], percentile: usize) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let index = ((samples.len() - 1) * percentile).div_ceil(100);
    samples[index]
}

fn write_replaceable_file(path: &Path, contents: &[u8]) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("output path requires a parent directory")?;
    fs::create_dir_all(parent)?;
    if let Ok(metadata) = path.symlink_metadata()
        && (!metadata.file_type().is_file() || metadata.file_type().is_symlink())
    {
        return Err("benchmark output must be a regular file".into());
    }
    let file_name = path.file_name().ok_or("output path requires a file name")?.to_string_lossy();
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let temporary = parent.join(format!(".{file_name}.{}.{nonce}.tmp", std::process::id()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

fn fingerprint_tree(root: &Path) -> Result<String, Box<dyn Error>> {
    let files = tree_files(root)?;
    if files.is_empty() {
        return Err("model cache is empty after initialization".into());
    }
    let mut hash = Sha256::new();
    for path in files {
        let relative = path.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
        hash.update(relative.as_bytes());
        hash.update([0]);
        let mut file = File::open(path)?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn tree_size(root: &Path) -> Result<u64, Box<dyn Error>> {
    tree_files(root)?.into_iter().try_fold(0_u64, |total, path| {
        Ok(total.checked_add(fs::metadata(path)?.len()).ok_or("model cache size overflowed")?)
    })
}

fn tree_files(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let canonical_root = root.canonicalize()?;
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                let target = entry.path().canonicalize()?;
                if !target.starts_with(&canonical_root) || !target.is_file() {
                    return Err("model cache link escapes its dedicated root".into());
                }
                continue;
            }
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::{RetrievalSample, evaluate_retrieval, write_replaceable_file};
    use semantic_engine_core::Decision;
    use semantic_engine_vectors::VectorPolicy;
    use std::{fs, time::SystemTime};

    #[test]
    fn multi_target_metrics_count_negative_and_wrong_target_acceptances() {
        let samples = vec![
            RetrievalSample {
                expected: Decision::Rejected,
                expected_target_id: None,
                lexical: Decision::Rejected,
                lexical_target_id: None,
                candidate_target_id: "other".into(),
                score: 0.95,
                runner_up_score: Some(0.80),
            },
            RetrievalSample {
                expected: Decision::Accepted,
                expected_target_id: Some("expected".into()),
                lexical: Decision::Accepted,
                lexical_target_id: Some("expected".into()),
                candidate_target_id: "other".into(),
                score: 0.95,
                runner_up_score: Some(0.80),
            },
            RetrievalSample {
                expected: Decision::Accepted,
                expected_target_id: Some("expected".into()),
                lexical: Decision::Accepted,
                lexical_target_id: Some("expected".into()),
                candidate_target_id: "expected".into(),
                score: 0.95,
                runner_up_score: Some(0.80),
            },
        ];

        let metrics = evaluate_retrieval(&samples, VectorPolicy::default());

        assert_eq!(metrics.annotations, 3);
        assert_eq!(metrics.positive_annotations, 2);
        assert_eq!(metrics.top1_correct, 1);
        assert_eq!(metrics.accepted_correct, 1);
        assert_eq!(metrics.accepted_wrong, 2);
        assert_eq!(metrics.false_accepts, 2);
        assert_eq!(metrics.decision_accuracy, 1.0 / 3.0);
        assert_eq!(metrics.confusion["rejected->accepted"], 1);
        assert_eq!(metrics.confusion["accepted->accepted_wrong_target"], 1);
    }

    #[test]
    fn report_output_can_be_replaced_after_a_complete_temporary_write() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock should be after the Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir()
            .join(format!("semantic-engine-vector-benchmark-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).expect("test directory should be created");
        let output = directory.join("report.json");

        write_replaceable_file(&output, br#"{"version":1}"#)
            .expect("first report should be written");
        write_replaceable_file(&output, br#"{"version":2}"#).expect("report should be replaceable");

        assert_eq!(fs::read(&output).expect("report should be readable"), br#"{"version":2}"#);
        fs::remove_dir_all(&directory).expect("owned test directory should be removable");
    }
}
