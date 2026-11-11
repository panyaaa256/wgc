//! Command-line interface for `wgc`.

mod args;
mod messages;
mod ui;

use std::collections::BTreeSet;
use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use clap::Parser;
use wgc_core::layout::{self, WorldLayout};
use wgc_core::output::{self, GcOptions, OutputError, VerifyError};
use wgc_core::plan::{PlanError, PlanOptions, build_plan};
use wgc_core::report::{self, LockStatus, ReportError};
use wgc_core::trust::check_trust;

use args::{Cli, Command, GcArgs, ScanArgs, VerifyArgs};
use ui::{SummaryKind, Terminal};

/// Process exit codes.
mod exit {
    /// Also used when there were warnings, unless `--strict` was given.
    pub const SUCCESS: u8 = 0;
    /// There were warnings and `--strict` was given.
    pub const WARNINGS: u8 = 1;
    pub const USAGE: u8 = 2;
    /// Stopped before writing anything.
    pub const STOPPED: u8 = 3;
    pub const ERROR: u8 = 4;
    /// The output failed verification, or is not ready to swap in.
    pub const VERIFY: u8 = 5;
    pub const INTERRUPTED: u8 = 130;
}

static CANCEL: AtomicBool = AtomicBool::new(false);
static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

fn main() -> ExitCode {
    let cli = Cli::parse();
    // First Ctrl-C asks the core to stop; the second exits at once.
    let _ = ctrlc::set_handler(|| {
        if INTERRUPTS.fetch_add(1, Ordering::SeqCst) > 0 {
            std::process::exit(i32::from(exit::INTERRUPTED));
        }
        CANCEL.store(true, Ordering::SeqCst);
    });
    let term = Terminal::new(cli.quiet, cli.no_color);
    let code = match &cli.command {
        Command::Scan(a) => scan(a, &cli, &term),
        Command::Gc(a) => gc(a, &cli, &term),
        Command::Verify(a) => verify(a, &cli, &term),
    };
    ExitCode::from(code)
}

/// Where the human-readable summary goes: stdout, or stderr with `--json`, or nowhere with `--quiet`.
fn summary_sink(json: bool, quiet: bool) -> Option<Box<dyn Write>> {
    match (quiet, json) {
        (true, _) => None,
        (false, true) => Some(Box::new(std::io::stderr())),
        (false, false) => Some(Box::new(std::io::stdout())),
    }
}

/// Colors, terminal width and verbosity for the stream [`summary_sink`] picks.
fn sink_format(json: bool, cli: &Cli) -> ui::SummaryFormat {
    let term = if json {
        console::Term::stderr()
    } else {
        console::Term::stdout()
    };
    let tty = term.is_term();
    ui::SummaryFormat {
        style: ui::Style {
            color: ui::color_enabled(cli.no_color, tty),
        },
        width: tty
            .then(|| term.size_checked())
            .flatten()
            .map(|(_, cols)| usize::from(cols)),
        verbose: cli.verbose,
    }
}

fn print_json(value: &impl serde::Serialize, term: &Terminal) -> Result<(), u8> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)
        .and_then(|()| writeln!(stdout).map_err(serde_json::Error::io))
        .map_err(|e| {
            term.error(&format!("cannot write JSON: {e}"));
            exit::ERROR
        })
}

fn detect(path: &std::path::Path, command: &str, term: &Terminal) -> Result<WorldLayout, u8> {
    layout::detect(path).map_err(|e| {
        term.error(&messages::layout(&e, command));
        exit::ERROR
    })
}

fn report_error(e: &ReportError, command: &str, term: &Terminal) -> u8 {
    term.error(&messages::report(e, command));
    match e {
        ReportError::Plan(PlanError::Cancelled) => exit::INTERRUPTED,
        _ => exit::ERROR,
    }
}

fn finish(warned: bool, strict: bool) -> u8 {
    if warned && strict {
        exit::WARNINGS
    } else {
        exit::SUCCESS
    }
}

/// A Ctrl-C that arrived after the core returned still ends the run with 130,
/// before any result is printed.
fn interrupted(term: &Terminal) -> Option<u8> {
    CANCEL.load(Ordering::SeqCst).then(|| {
        term.error(messages::INTERRUPTED);
        exit::INTERRUPTED
    })
}

fn scan(a: &ScanArgs, cli: &Cli, term: &Terminal) -> u8 {
    let layout = match detect(&a.path, "scan", term) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let report = match report::run_scan(&layout, term, &CANCEL) {
        Ok(r) => r,
        Err(e) => return report_error(&e, "scan", term),
    };
    if let Some(code) = interrupted(term) {
        return code;
    }
    if let Some(mut out) = summary_sink(a.json, cli.quiet) {
        let _ = ui::summary(
            &mut out,
            &report,
            &layout.root,
            SummaryKind::Scan,
            sink_format(a.json, cli),
        );
    }
    if a.json
        && let Err(code) = print_json(&report, term)
    {
        return code;
    }
    finish(term.warnings() > 0 || report.has_stops(), a.strict)
}

fn gc(a: &GcArgs, cli: &Cli, term: &Terminal) -> u8 {
    let layout = match detect(&a.path, "gc", term) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let gc_dims: Option<BTreeSet<_>> =
        (!a.dimensions.is_empty()).then(|| a.dimensions.iter().cloned().collect());
    if let Some(dims) = &gc_dims {
        let known: BTreeSet<_> = layout.dimensions.iter().map(|d| &d.id).collect();
        if let Some(missing) = dims.iter().find(|d| !known.contains(d)) {
            term.error(&messages::unknown_dimension(missing.as_str()));
            return exit::USAGE;
        }
    }
    match &a.out {
        None => gc_estimate(a, cli, term, &layout, gc_dims),
        Some(out) => gc_output(a, cli, term, &layout, gc_dims, out),
    }
}

fn gc_estimate(
    a: &GcArgs,
    cli: &Cli,
    term: &Terminal,
    layout: &WorldLayout,
    gc_dims: Option<BTreeSet<wgc_core::types::DimensionId>>,
) -> u8 {
    let (lock, _held) = report::try_lock_for_report(layout, term);
    let plan = match build_plan(layout, &PlanOptions { gc_dims }, term, &CANCEL) {
        Ok(p) => p,
        Err(e) => return report_error(&ReportError::Plan(e), "gc", term),
    };
    let trust = check_trust(&plan);
    let result = report::scan_report(&plan, &trust, lock)
        .and_then(|scan| report::gc_report(scan, None, &BTreeSet::new(), &plan));
    let report = match result {
        Ok(r) => r,
        Err(e) => return report_error(&e, "gc", term),
    };
    if let Some(code) = interrupted(term) {
        return code;
    }
    if let Some(mut out) = summary_sink(a.json, cli.quiet) {
        let _ = ui::summary(
            &mut out,
            &report.scan,
            &layout.root,
            SummaryKind::Estimate,
            sink_format(a.json, cli),
        );
        let _ = writeln!(out, "{}", messages::NOTHING_WRITTEN);
    }
    if a.json
        && let Err(code) = print_json(&report, term)
    {
        return code;
    }
    finish(term.warnings() > 0 || report.scan.has_stops(), a.strict)
}

fn gc_output(
    a: &GcArgs,
    cli: &Cli,
    term: &Terminal,
    layout: &WorldLayout,
    gc_dims: Option<BTreeSet<wgc_core::types::DimensionId>>,
    out: &std::path::Path,
) -> u8 {
    let options = GcOptions {
        out: out.to_path_buf(),
        gc_dims,
        allowed: a
            .force
            .as_ref()
            .map(args::Force::allowed)
            .unwrap_or_default(),
    };
    let outcome = match output::run_gc(layout, &options, term, &CANCEL) {
        Ok(o) => o,
        Err(e) => {
            term.error(&messages::output(&e));
            return match e {
                OutputError::OutputNotEmpty { .. }
                | OutputError::InvalidOutputPath { .. }
                | OutputError::OutputOverlapsSource { .. }
                | OutputError::Stopped(_)
                | OutputError::InsufficientSpace { .. } => exit::STOPPED,
                OutputError::Lock(wgc_core::lock::LockError::Io { .. }) => exit::ERROR,
                OutputError::Lock(_) => exit::STOPPED,
                OutputError::Verification(_) => exit::VERIFY,
                OutputError::Cancelled => exit::INTERRUPTED,
                OutputError::Layout(_) | OutputError::Plan(_) | OutputError::Io { .. } => {
                    exit::ERROR
                }
            };
        }
    };
    // When the missing session.lock stop (L-1) was forced, the output folders were not locked.
    let output_lock = if outcome.lock_skipped {
        LockStatus::Missing
    } else {
        LockStatus::Acquired
    };
    let result = report::scan_report(&outcome.plan, &outcome.trust, output_lock).and_then(|scan| {
        report::gc_report(scan, Some(&outcome.out), &outcome.forced, &outcome.plan)
    });
    let report = match result {
        Ok(r) => r,
        Err(e) => return report_error(&e, "gc", term),
    };
    if let Some(mut sink) = summary_sink(a.json, cli.quiet) {
        let format = sink_format(a.json, cli);
        let style = format.style;
        let _ = ui::summary(
            &mut sink,
            &report.scan,
            &layout.root,
            SummaryKind::Output,
            format,
        );
        let _ = writeln!(
            sink,
            "{} {}",
            style.green(messages::DONE),
            outcome.out.display()
        );
        if !outcome.forced.is_empty() {
            let ids: Vec<String> = outcome.forced.iter().map(ToString::to_string).collect();
            let _ = writeln!(sink, "{}", style.yellow(&messages::forced_output(&ids)));
        }
        let _ = ui::swap_steps(
            &mut sink,
            layout.kind.as_str(),
            &layout.root,
            &ui::layout_paths(layout),
            &outcome.out,
        );
    }
    if a.json
        && let Err(code) = print_json(&report, term)
    {
        return code;
    }
    finish(term.warnings() > 0, a.strict)
}

#[derive(serde::Serialize)]
struct VerifyJson {
    schema: &'static str,
    result: &'static str,
    source: Option<String>,
    changed: Vec<String>,
    forced: Vec<String>,
}

fn verify(a: &VerifyArgs, cli: &Cli, term: &Terminal) -> u8 {
    let result = output::verify(&a.out, a.source.as_deref());
    if let Some(code) = interrupted(term) {
        return code;
    }
    let (name, code) = match &result {
        Ok(_) => ("ready", exit::SUCCESS),
        Err(VerifyError::Incomplete { .. }) => ("incomplete", exit::VERIFY),
        Err(VerifyError::SourceMismatch { .. }) => ("source_mismatch", exit::VERIFY),
        Err(VerifyError::Changed(_)) => ("changed", exit::VERIFY),
        Err(VerifyError::InUse { .. }) => ("in_use", exit::VERIFY),
        Err(VerifyError::Io { .. }) => ("error", exit::ERROR),
    };
    match &result {
        Ok(ok) => {
            if let Some(mut sink) = summary_sink(a.json, cli.quiet) {
                let style = sink_format(a.json, cli).style;
                let _ = writeln!(sink, "{}", style.green(messages::READY));
                if !ok.forced.is_empty() {
                    let _ = writeln!(
                        sink,
                        "{}",
                        style.yellow(&messages::forced_output(&ok.forced))
                    );
                }
                let _ =
                    ui::swap_steps(&mut sink, &ok.layout, &ok.source, &ok.world_folders, &a.out);
            }
        }
        Err(e) => term.error(&messages::verify(e)),
    }
    if a.json {
        let json = VerifyJson {
            schema: "wgc.verify/1",
            result: name,
            source: result.as_ref().ok().map(|o| o.source.display().to_string()),
            changed: match &result {
                Err(VerifyError::Changed(c)) => c
                    .iter()
                    .map(|c| match c {
                        output::ChangedFile::Added(p)
                        | output::ChangedFile::Removed(p)
                        | output::ChangedFile::Modified(p) => p.clone(),
                    })
                    .collect(),
                _ => Vec::new(),
            },
            forced: result
                .as_ref()
                .map(|o| o.forced.clone())
                .unwrap_or_default(),
        };
        if let Err(c) = print_json(&json, term) {
            return c;
        }
    }
    code
}
