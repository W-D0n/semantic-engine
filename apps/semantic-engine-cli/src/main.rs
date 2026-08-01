use std::{
    env,
    error::Error,
    fs,
    io::{self, BufRead, Write},
    process::ExitCode,
    time::Instant,
};

use semantic_engine_core::{Round, Submission, ValidationPolicy, Validator};

use semantic_engine_package::import_package;
use semantic_engine_protocol::{handle_json_line, line_too_large_response};
use semantic_engine_service::{SemanticEngineService, ServiceConfig};

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
        if args.next().as_deref() != Some("validate") || args.next().as_deref() != Some("--package")
        {
            return Err(
                "usage: semantic-engine-cli context validate --package <datapackage.json>".into()
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
        return Ok(());
    }

    if command.as_deref() == Some("benchmark") {
        return run_benchmark(args);
    }

    if command.as_deref() == Some("serve") {
        return run_server(args);
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
         semantic-engine-cli benchmark (--round <round.json> | --package <datapackage.json>) --submissions <submissions.jsonl> [--iterations <1..1000>]\n  \
         semantic-engine-cli serve [--audit <audit.sqlite3>]\n\n\
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
