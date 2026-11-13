//! `langhost`: compile, verify and run one unit on the host.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use abi::ExitReason;

const USAGE: &str = "\
usage: langhost [options] <file.ig>

Compiles one igis-v2 unit, verifies the package and runs it.
Program output goes to stdout; diagnostics go to stderr.

options:
  --emit-package <path>  write the package container to <path>
  --args <text>          text delivered to main(args); at most 1024 bytes
  -h, --help             print this help
";

fn main() -> ExitCode {
    let mut emit: Option<PathBuf> = None;
    let mut arguments = String::new();
    let mut file: Option<PathBuf> = None;

    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--emit-package" => match args.next() {
                Some(path) => emit = Some(PathBuf::from(path)),
                None => {
                    eprintln!("langhost: --emit-package needs a path");
                    return ExitCode::FAILURE;
                }
            },
            "--args" => match args.next() {
                Some(text) => arguments = text,
                None => {
                    eprintln!("langhost: --args needs a value");
                    return ExitCode::FAILURE;
                }
            },
            other if other.starts_with('-') => {
                eprintln!("langhost: unknown option `{other}`");
                eprint!("{USAGE}");
                return ExitCode::FAILURE;
            }
            other => {
                if file.is_some() {
                    eprintln!("langhost: more than one input file");
                    return ExitCode::FAILURE;
                }
                file = Some(PathBuf::from(other));
            }
        }
    }

    let Some(file) = file else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let display = file.display().to_string();
    let source = match fs::read(&file) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("langhost: {display}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let package = match langhost::compile(&source, &display) {
        Ok(package) => package,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(path) = emit {
        if let Err(error) = langhost::write_package(&path, &package) {
            eprintln!("langhost: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    }

    let result = match langhost::run(
        &package.code,
        arguments.as_bytes(),
        langhost::DEFAULT_BUDGET,
    ) {
        Ok(result) => result,
        Err(diagnostic) => {
            eprintln!("{display}: {diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = io::stdout().write_all(&result.output) {
        if error.kind() != io::ErrorKind::BrokenPipe {
            eprintln!("langhost: stdout: {error}");
            return ExitCode::FAILURE;
        }
    }

    match result.reason {
        ExitReason::Ok => ExitCode::SUCCESS,
        ExitReason::Killed => {
            eprintln!("langhost: {display}: task was killed");
            ExitCode::FAILURE
        }
        ExitReason::Faulted(code) => {
            match result.fault_at {
                Some((line, column)) => {
                    eprintln!("{display}:{line}:{column}: fault {}", code.name())
                }
                None => eprintln!("{display}: fault {}", code.name()),
            }
            ExitCode::FAILURE
        }
    }
}
