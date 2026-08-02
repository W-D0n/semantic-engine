use std::{
    collections::{BTreeMap, HashSet},
    env,
    error::Error,
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Instant,
};

use semantic_engine_context_index::{inspect_channel_root_now, inspect_offline_channel_now};
use semantic_engine_core::{
    AnswerTarget, Decision, Round, Submission, ValidationPolicy, Validator,
};
use serde::Deserialize;

use semantic_engine_loopback::{
    DEFAULT_PORT, LoopbackConfig, start_shared_with_sources as start_loopback,
};
use semantic_engine_package::import_package;
use semantic_engine_protocol::{handle_json_line, line_too_large_response};
use semantic_engine_service::{SemanticEngineService, ServiceConfig};
use semantic_engine_source_runtime::SourceRuntime;

const MAX_PROTOCOL_LINE_BYTES: usize = 1024 * 1024;
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("semantic-engine: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let command = args.next();

    if matches!(command.as_deref(), None | Some("--help" | "-h")) {
        print_help();
        return Ok(());
    }

    if command.as_deref() == Some("context") {
        return run_context(args);
    }

    if command.as_deref() == Some("benchmark") {
        return run_benchmark(args);
    }

    if command.as_deref() == Some("evaluate") {
        return run_evaluation(args);
    }

    if command.as_deref() == Some("serve") {
        return run_server(args);
    }

    if command.as_deref() == Some("loopback") {
        return run_loopback(args);
    }

    if command.as_deref() != Some("validate") || args.next().as_deref() != Some("--round") {
        return Err("usage: semantic-engine-cli validate --round <round.json>".into());
    }

    let round_path = args.next().ok_or("missing round JSON path")?;
    if args.next().is_some() {
        return Err("unexpected arguments after round JSON path".into());
    }

    let round: Round = serde_json::from_str(&fs::read_to_string(round_path)?)?;
    let validator = Validator::default();
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for (line_number, line) in stdin.lock().lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let submission: Submission = serde_json::from_str(&line).map_err(|error| {
            format!("invalid submission JSON on input line {}: {error}", line_number + 1)
        })?;
        let validation = validator.validate(&round, &submission);
        serde_json::to_writer(&mut stdout, &validation)?;
        writeln!(stdout)?;
        stdout.flush()?;
    }

    Ok(())
}

fn run_context(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    match args.next().as_deref() {
        Some("validate") => {
            if args.next().as_deref() != Some("--package") {
                return Err(
                    "usage: semantic-engine-cli context validate --package <datapackage.json>"
                        .into(),
                );
            }
            let package_path = args.next().ok_or("missing datapackage.json path")?;
            if args.next().is_some() {
                return Err("unexpected arguments after package path".into());
            }
            let imported = import_package(package_path)?;
            println!(
                "{}",
                serde_json::json!({
                    "status": "valid",
                    "name": imported.name,
                    "id": imported.id,
                    "version": imported.version.to_string(),
                    "package_sha256": imported.package_sha256,
                    "targets_sha256": imported.targets_sha256,
                    "sources": imported.sources,
                    "locales": imported.locales,
                    "license": imported.spdx_license_expression,
                    "targets": imported.targets.len()
                })
            );
        }
        Some("channel") => match args.next().as_deref() {
            Some("inspect-root") => {
                if args.next().as_deref() != Some("--root") {
                    return Err("usage: semantic-engine-cli context channel inspect-root --root <root.json>".into());
                }
                let root = args.next().ok_or("missing trusted root path")?;
                if args.next().is_some() {
                    return Err("unexpected arguments after trusted root path".into());
                }
                println!("{}", serde_json::to_string(&inspect_channel_root_now(root)?)?);
            }
            Some("verify") => {
                let channel = required_flag(&mut args, "--channel", "missing channel directory")?;
                let root = required_flag(&mut args, "--root", "missing trusted root path")?;
                let state = required_flag(&mut args, "--state", "missing channel state directory")?;
                if args.next().is_some() {
                    return Err("unexpected arguments after channel state directory".into());
                }
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                let verified =
                    runtime.block_on(inspect_offline_channel_now(channel, root, state))?;
                println!("{}", serde_json::to_string(&verified)?);
            }
            _ => {
                return Err("usage: semantic-engine-cli context channel (inspect-root --root <root.json> | verify --channel <directory> --root <root.json> --state <directory>)".into());
            }
        },
        _ => {
            return Err("usage: semantic-engine-cli context (validate --package <datapackage.json> | channel ...)".into());
        }
    }
    Ok(())
}

fn required_flag(
    args: &mut impl Iterator<Item = String>,
    expected: &str,
    missing: &'static str,
) -> Result<String, Box<dyn Error>> {
    if args.next().as_deref() != Some(expected) {
        return Err(format!("expected {expected}").into());
    }
    args.next().ok_or_else(|| missing.into())
}

fn run_server(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let mut service = match args.next() {
        None => SemanticEngineService::in_memory()?,
        Some(flag) if flag == "--audit" => {
            let path = args.next().ok_or("missing audit database path")?;
            if args.next().is_some() {
                return Err("unexpected arguments after audit database path".into());
            }
            SemanticEngineService::open(path)?
        }
        Some(_) => return Err("usage: semantic-engine-cli serve [--audit <audit.sqlite3>]".into()),
    };
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut line = Vec::new();
    let mut stdout = io::stdout().lock();

    loop {
        let response = match read_bounded_line(&mut input, &mut line, MAX_PROTOCOL_LINE_BYTES)? {
            BoundedLine::Eof => break,
            BoundedLine::TooLarge => line_too_large_response(),
            BoundedLine::Line if line.iter().all(u8::is_ascii_whitespace) => continue,
            BoundedLine::Line => handle_json_line(&mut service, &line),
        };
        serde_json::to_writer(&mut stdout, &response)?;
        writeln!(stdout)?;
        stdout.flush()?;
    }
    Ok(())
}

fn run_loopback(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    if args.next().as_deref() != Some("--enable") || args.next().as_deref() != Some("--audit") {
        return Err(loopback_usage().into());
    }
    let audit_path = PathBuf::from(args.next().ok_or("missing loopback audit database path")?);
    let mut sources_path = None;
    let mut config = LoopbackConfig::default();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--port" => {
                let port = args
                    .next()
                    .ok_or("missing loopback port")?
                    .parse::<u16>()
                    .map_err(|_| "loopback port must be between 0 and 65535")?;
                config.bind_addr.set_port(port);
            }
            "--origin" => {
                let origin = args.next().ok_or("missing allowed browser origin")?;
                config.allowed_origins.push(origin);
            }
            "--sources" => {
                sources_path = Some(PathBuf::from(args.next().ok_or("missing sources database")?));
            }
            _ => return Err(loopback_usage().into()),
        }
    }
    let sources_path = sources_path.unwrap_or_else(|| audit_path.with_extension("sources.sqlite3"));
    let service = Arc::new(tokio::sync::Mutex::new(SemanticEngineService::open(&audit_path)?));
    let sources = Arc::new(SourceRuntime::open(sources_path, service.clone())?);
    tokio::runtime::Runtime::new()?.block_on(async move {
        let server = start_loopback(service, sources, config).await?;
        println!(
            "{}",
            serde_json::json!({
                "status": "ready",
                "address": format!("http://{}", server.addr()),
                "token": server.token(),
                "protocol_version": semantic_engine_protocol::PROTOCOL_VERSION,
            })
        );
        io::stdout().flush()?;
        tokio::signal::ctrl_c().await?;
        server.shutdown().await?;
        Ok::<(), Box<dyn Error>>(())
    })
}

fn loopback_usage() -> String {
    format!(
        "usage: semantic-engine-cli loopback --enable --audit <state.sqlite3> [--sources <sources.sqlite3>] [--port <0..65535>] [--origin <origin>] (default port: {DEFAULT_PORT})"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundedLine {
    Eof,
    Line,
    TooLarge,
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    output: &mut Vec<u8>,
    max_bytes: usize,
) -> io::Result<BoundedLine> {
    output.clear();
    let mut saw_input = false;
    let mut too_large = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(if !saw_input {
                BoundedLine::Eof
            } else if too_large {
                BoundedLine::TooLarge
            } else {
                BoundedLine::Line
            });
        }
        saw_input = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let data_bytes = newline.unwrap_or(available.len());
        if !too_large {
            if output.len().saturating_add(data_bytes) > max_bytes {
                too_large = true;
                output.clear();
            } else {
                output.extend_from_slice(&available[..data_bytes]);
            }
        }
        let consumed = newline.map_or(available.len(), |position| position + 1);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(if too_large { BoundedLine::TooLarge } else { BoundedLine::Line });
        }
    }
}

fn run_benchmark(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let source_flag = args.next().ok_or_else(benchmark_usage)?;
    let source_path = args.next().ok_or("missing benchmark round or package path")?;
    let (round, context_package_sha256) = match source_flag.as_str() {
        "--round" => {
            let round = serde_json::from_str(&fs::read_to_string(source_path)?)?;
            (round, None)
        }
        "--package" => {
            let imported = import_package(source_path)?;
            let round = Round {
                id: format!("benchmark-{}-{}", imported.id, imported.version),
                targets: imported.targets,
                policy: ValidationPolicy::default(),
            };
            (round, Some(imported.package_sha256))
        }
        _ => return Err(benchmark_usage().into()),
    };
    if args.next().as_deref() != Some("--submissions") {
        return Err(benchmark_usage().into());
    }
    let submissions_path = args.next().ok_or("missing submissions JSONL path")?;
    let iterations = match args.next() {
        None => 100,
        Some(flag) if flag == "--iterations" => args
            .next()
            .ok_or("missing benchmark iteration count")?
            .parse::<usize>()
            .map_err(|_| "benchmark iteration count must be an integer")?,
        Some(_) => return Err(benchmark_usage().into()),
    };
    if args.next().is_some() || !(1..=1_000).contains(&iterations) {
        return Err("benchmark iterations must be between 1 and 1000".into());
    }

    let submission_lines = fs::read_to_string(submissions_path)?;
    let submissions = submission_lines
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<Submission>)
        .collect::<Result<Vec<_>, _>>()?;
    if submissions.is_empty() || submissions.len() > 10_000 {
        return Err("benchmark submissions must contain between 1 and 10000 records".into());
    }
    let sample_count =
        iterations.checked_mul(submissions.len()).ok_or("benchmark sample count overflowed")?;
    if sample_count > 1_000_000 {
        return Err("benchmark is limited to 1000000 samples per measured path".into());
    }

    let validator = Validator::default();
    let mut engine_timings = Vec::with_capacity(sample_count);
    for _ in 0..iterations {
        for submission in &submissions {
            let started = Instant::now();
            let _ = validator.validate(&round, submission);
            engine_timings.push(elapsed_ns(started));
        }
    }

    let mut service = SemanticEngineService::in_memory()?;
    for (index, submission) in submissions.iter().enumerate() {
        let mut warmup = submission.clone();
        warmup.message_id = format!("benchmark-warmup-{index}");
        warmup.source_sequence = u64::try_from(index)?;
        service.validate(round.clone(), warmup, context_package_sha256.as_deref())?;
    }
    let mut uncached_service = SemanticEngineService::in_memory_with_config(ServiceConfig {
        cache_capacity: 0,
        ..ServiceConfig::default()
    })?;
    let mut uncached_service_timings = Vec::with_capacity(sample_count);
    for iteration in 0..iterations {
        for (index, submission) in submissions.iter().enumerate() {
            let mut request = submission.clone();
            request.message_id = format!("benchmark-uncached-{iteration}-{index}");
            request.source_sequence = u64::try_from(iteration * submissions.len() + index)?;
            let started = Instant::now();
            uncached_service.validate(round.clone(), request, context_package_sha256.as_deref())?;
            uncached_service_timings.push(elapsed_ns(started));
        }
    }
    let mut cached_timings = Vec::with_capacity(sample_count);
    for iteration in 0..iterations {
        for (index, submission) in submissions.iter().enumerate() {
            let mut request = submission.clone();
            request.message_id = format!("benchmark-{iteration}-{index}");
            request.source_sequence = u64::try_from(iteration * submissions.len() + index)?;
            let started = Instant::now();
            service.validate(round.clone(), request, context_package_sha256.as_deref())?;
            cached_timings.push(elapsed_ns(started));
        }
    }

    println!(
        "{}",
        serde_json::json!({
            "iterations": iterations,
            "targets": round.targets.len(),
            "submissions_per_iteration": submissions.len(),
            "samples": engine_timings.len(),
            "engine_lexical_only_ns": latency_summary(&mut engine_timings),
            "service_cache_disabled_ns": latency_summary(&mut uncached_service_timings),
            "service_warm_cache_ns": latency_summary(&mut cached_timings),
            "service_stats": service.stats()
        })
    );
    Ok(())
}

fn benchmark_usage() -> &'static str {
    "usage: semantic-engine-cli benchmark (--round <round.json> | --package <datapackage.json>) --submissions <submissions.jsonl> [--iterations <1..1000>]"
}

#[derive(Deserialize)]
struct EvaluationTitles {
    version: u32,
    titles: Vec<EvaluationTitle>,
}

#[derive(Deserialize)]
struct EvaluationTitle {
    id: String,
    canonical: String,
    aliases: Vec<String>,
}

#[derive(Deserialize)]
struct EvaluationCases {
    version: u32,
    cases: Vec<EvaluationCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvaluationCase {
    target_id: String,
    input: String,
    expected: String,
    category: String,
}

#[derive(Default)]
struct CategoryResult {
    annotations: u64,
    correct: u64,
}

fn run_evaluation(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    if args.next().as_deref() != Some("--titles") {
        return Err(evaluation_usage().into());
    }
    let titles_path = args.next().ok_or("missing evaluation titles path")?;
    if args.next().as_deref() != Some("--cases") {
        return Err(evaluation_usage().into());
    }
    let cases_path = args.next().ok_or("missing evaluation cases path")?;
    let mut minimum_precision = 0.95;
    let mut minimum_recall = 0.90;
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or("evaluation threshold flag requires a value")?
            .parse::<f64>()
            .map_err(|_| "evaluation thresholds must be numbers between 0 and 1")?;
        match flag.as_str() {
            "--minimum-precision" => minimum_precision = value,
            "--minimum-recall" => minimum_recall = value,
            _ => return Err(evaluation_usage().into()),
        }
    }
    if [minimum_precision, minimum_recall]
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(evaluation_usage().into());
    }

    let titles: EvaluationTitles = serde_json::from_str(&fs::read_to_string(titles_path)?)?;
    let cases: EvaluationCases = serde_json::from_str(&fs::read_to_string(cases_path)?)?;
    if titles.titles.is_empty() || titles.titles.len() > 10_000 {
        return Err("evaluation titles must contain between 1 and 10000 records".into());
    }
    if cases.cases.is_empty() || cases.cases.len() > 10_000 {
        return Err("evaluation cases must contain between 1 and 10000 annotations".into());
    }
    let mut title_ids = HashSet::with_capacity(titles.titles.len());
    if titles.titles.iter().any(|title| !title_ids.insert(title.id.as_str())) {
        return Err("evaluation title identifiers must be unique".into());
    }

    let validator = Validator::default();
    let mut correct = 0_u64;
    let mut actual_accepts = 0_u64;
    let mut expected_accepts = 0_u64;
    let mut true_accepts = 0_u64;
    let mut false_accepts = 0_u64;
    let mut confusion = BTreeMap::<String, u64>::new();
    let mut categories = BTreeMap::<String, CategoryResult>::new();

    for (sequence, case) in cases.cases.iter().enumerate() {
        if case.category.is_empty() || case.category.len() > 64 {
            return Err("evaluation case category is invalid".into());
        }
        let expected = parse_decision(&case.expected)?;
        let title = titles
            .titles
            .iter()
            .find(|title| title.id == case.target_id)
            .ok_or("evaluation case refers to a missing title")?;
        let round = Round {
            id: format!("evaluation-{}", title.id),
            targets: vec![AnswerTarget {
                id: title.id.clone(),
                canonical: title.canonical.clone(),
                aliases: title.aliases.clone(),
            }],
            policy: ValidationPolicy::default(),
        };
        let validation = validator.validate(
            &round,
            &Submission {
                message_id: format!("evaluation-{sequence}"),
                participant_id: "evaluation-runner".to_owned(),
                source_sequence: u64::try_from(sequence)?,
                text: case.input.clone(),
            },
        );
        let accepted_target_is_correct = validation.decision != Decision::Accepted
            || validation.target_id.as_deref() == Some(title.id.as_str());
        let case_correct = validation.decision == expected && accepted_target_is_correct;
        correct += u64::from(case_correct);
        expected_accepts += u64::from(expected == Decision::Accepted);
        actual_accepts += u64::from(validation.decision == Decision::Accepted);
        let true_accept = expected == Decision::Accepted
            && validation.decision == Decision::Accepted
            && accepted_target_is_correct;
        true_accepts += u64::from(true_accept);
        false_accepts += u64::from(validation.decision == Decision::Accepted && !true_accept);
        *confusion
            .entry(format!("{}->{}", decision_name(&expected), decision_name(&validation.decision)))
            .or_default() += 1;
        let category = categories.entry(case.category.clone()).or_default();
        category.annotations += 1;
        category.correct += u64::from(case_correct);
    }

    let annotations = u64::try_from(cases.cases.len())?;
    let accepted_precision = ratio(true_accepts, actual_accepts);
    let accepted_recall = ratio(true_accepts, expected_accepts);
    let decision_accuracy = ratio(correct, annotations);
    let gate_passed = accepted_precision >= minimum_precision && accepted_recall >= minimum_recall;
    let category_report = categories
        .into_iter()
        .map(|(name, result)| {
            (
                name,
                serde_json::json!({
                    "annotations": result.annotations,
                    "correct": result.correct,
                    "accuracy": ratio(result.correct, result.annotations),
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    println!(
        "{}",
        serde_json::json!({
            "engine": "lexical-v1",
            "title_corpus_version": titles.version,
            "case_corpus_version": cases.version,
            "titles": titles.titles.len(),
            "annotations": annotations,
            "accepted_precision": accepted_precision,
            "accepted_recall": accepted_recall,
            "decision_accuracy": decision_accuracy,
            "false_accepts": false_accepts,
            "minimum_precision": minimum_precision,
            "minimum_recall": minimum_recall,
            "gate_passed": gate_passed,
            "confusion": confusion,
            "categories": category_report,
        })
    );
    if !gate_passed {
        return Err(format!(
            "quality gate failed: precision {accepted_precision:.4}/{minimum_precision:.4}, recall {accepted_recall:.4}/{minimum_recall:.4}"
        )
        .into());
    }
    Ok(())
}

fn parse_decision(value: &str) -> Result<Decision, Box<dyn Error>> {
    match value {
        "accepted" => Ok(Decision::Accepted),
        "abstained" => Ok(Decision::Abstained),
        "rejected" => Ok(Decision::Rejected),
        _ => Err("evaluation expected decision is invalid".into()),
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

fn evaluation_usage() -> &'static str {
    "usage: semantic-engine-cli evaluate --titles <titles.json> --cases <cases.json> [--minimum-precision <0..1>] [--minimum-recall <0..1>]"
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn latency_summary(samples: &mut [u64]) -> serde_json::Value {
    samples.sort_unstable();
    serde_json::json!({
        "p50": percentile(samples, 50),
        "p95": percentile(samples, 95),
        "p99": percentile(samples, 99),
        "max": samples.last().copied().unwrap_or(0)
    })
}

fn percentile(samples: &[u64], percentile: usize) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let rank = (percentile * samples.len()).div_ceil(100).saturating_sub(1);
    samples[rank.min(samples.len() - 1)]
}

fn print_help() {
    println!(
        "Semantic Engine local tools\n\n\
         Usage:\n  semantic-engine-cli validate --round <round.json>\n  \
         semantic-engine-cli context validate --package <datapackage.json>\n  \
         semantic-engine-cli context channel inspect-root --root <root.json>\n  \
         semantic-engine-cli context channel verify --channel <directory> --root <root.json> --state <directory>\n  \
         semantic-engine-cli benchmark (--round <round.json> | --package <datapackage.json>) --submissions <submissions.jsonl> [--iterations <1..1000>]\n  \
         semantic-engine-cli evaluate --titles <titles.json> --cases <cases.json> [--minimum-precision <0..1>] [--minimum-recall <0..1>]\n  \
         semantic-engine-cli serve [--audit <audit.sqlite3>]\n  \
         semantic-engine-cli loopback --enable --audit <state.sqlite3> [--sources <sources.sqlite3>] [--port <0..65535>] [--origin <origin>]\n\n\
         Reads one Submission JSON object per stdin line and immediately writes one Validation JSON object."
    );
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{BoundedLine, percentile, read_bounded_line};

    #[test]
    fn percentile_uses_nearest_rank_without_reading_past_the_samples() {
        let samples = [1, 2, 3, 4, 5];
        assert_eq!(percentile(&samples, 50), 3);
        assert_eq!(percentile(&samples, 95), 5);
        assert_eq!(percentile(&samples, 99), 5);
        assert_eq!(percentile(&[], 99), 0);
    }

    #[test]
    fn bounded_protocol_reader_drains_an_oversized_line_and_recovers() {
        let mut reader = Cursor::new(b"123456\nok\n".to_vec());
        let mut line = Vec::new();
        assert_eq!(read_bounded_line(&mut reader, &mut line, 4).unwrap(), BoundedLine::TooLarge);
        assert!(line.is_empty());
        assert_eq!(read_bounded_line(&mut reader, &mut line, 4).unwrap(), BoundedLine::Line);
        assert_eq!(line, b"ok");
    }
}
