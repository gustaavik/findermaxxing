//! `cargo xtask ci`       — every gate CI runs, in the same order, locally.
//! `cargo xtask licenses` — third-party license report for distribution.
//! `cargo xtask package-macos [--universal] [--sign IDENTITY] [--notarize PROFILE]`,
//! `cargo xtask package-appimage` — release packages (see `package.rs`).

mod package;

use std::env;
use std::process::{Command, ExitCode};

const QMAKE_VARIABLE: &str = "QMAKE";
const LICENSE_REPORT: &str = "target/third-party-licenses.html";
const LICENSE_TEMPLATE: &str = "about.hbs";

#[derive(Debug)]
struct Step {
    name: &'static str,
    program: &'static str,
    args: Vec<String>,
}

fn main() -> ExitCode {
    let task = env::args().nth(1);
    let rest: Vec<String> = env::args().skip(2).collect();
    let packaged = match task.as_deref() {
        Some("package-macos") => Some(package::macos(&rest)),
        Some("package-appimage") => Some(package::appimage()),
        _ => None,
    };
    if let Some(outcome) = packaged {
        return outcome.map_or_else(
            |error| {
                eprintln!("xtask: {error}");
                ExitCode::FAILURE
            },
            |()| ExitCode::SUCCESS,
        );
    }
    let steps = match task.as_deref() {
        Some("ci") => ci_steps(),
        Some("licenses") => vec![license_step()],
        _ => {
            eprintln!("usage: cargo xtask <ci|licenses|package-macos|package-appimage>");
            return ExitCode::FAILURE;
        }
    };
    run_all(&steps)
}

/// Qt-dependent crates join the gates only when a Qt installation is configured.
fn workspace_scope() -> Vec<String> {
    if env::var_os(QMAKE_VARIABLE).is_some() {
        vec!["--workspace".to_owned()]
    } else {
        eprintln!("note: {QMAKE_VARIABLE} not set; skipping Qt crates (presentation, app)");
        Vec::new()
    }
}

fn ci_steps() -> Vec<Step> {
    let scope = workspace_scope();
    vec![
        Step { name: "format", program: "cargo", args: args(&["fmt", "--all", "--check"], &[]) },
        Step {
            name: "lint",
            program: "cargo",
            args: args(&["clippy", "--all-targets", "--locked"], &scope)
                .into_iter()
                .chain(args(&["--", "-D", "warnings"], &[]))
                .collect(),
        },
        Step { name: "test", program: "cargo", args: args(&["test", "--locked"], &scope) },
        Step { name: "dependency policy", program: "cargo", args: args(&["deny", "--workspace", "check"], &[]) },
    ]
}

fn license_step() -> Step {
    Step {
        name: "license report",
        program: "cargo",
        args: args(&["about", "generate", "--workspace", "--output-file", LICENSE_REPORT, LICENSE_TEMPLATE], &[]),
    }
}

fn args(fixed: &[&str], extra: &[String]) -> Vec<String> {
    fixed.iter().map(|s| (*s).to_owned()).chain(extra.iter().cloned()).collect()
}

fn run_all(steps: &[Step]) -> ExitCode {
    for step in steps {
        eprintln!("==> {}: {} {}", step.name, step.program, step.args.join(" "));
        match Command::new(step.program).args(&step.args).status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("xtask: step `{}` failed ({status})", step.name);
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("xtask: could not run `{}`: {error} (is the tool installed? see README)", step.program);
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
