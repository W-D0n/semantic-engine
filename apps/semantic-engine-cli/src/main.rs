use std::{
    env,
    error::Error,
    fs,
    io::{self, BufRead, Write},
    process::ExitCode,
};

use semantic_engine_core::{Round, Submission, Validator};

use semantic_engine_package::import_package;
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

fn print_help() {
    println!(
        "Semantic Engine local tools\n\n\
         Usage:\n  semantic-engine-cli validate --round <round.json>\n  \
         semantic-engine-cli context validate --package <datapackage.json>\n\n\
         Reads one Submission JSON object per stdin line and immediately writes one Validation JSON object."
    );
}
