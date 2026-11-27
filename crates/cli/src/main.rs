//! Command-line interface for `wgc`.

mod args;
mod messages;
mod ui;

use std::collections::BTreeSet;
use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use clap::Parser;
use wgc_fs::Outcome;
use wgc_fs::layout::{self, WorldLayout};
use wgc_fs::output::{self, GcOptions, OutputError, VerifyError, VerifyJson};
use wgc_fs::report::{self, ReportError};
use wgc_fs::world::{FsWorld, Rayon};

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

    /// The exit code for how a command ended.
    pub const fn code(outcome: wgc_fs::Outcome) -> u8 {
        use wgc_fs::Outcome;
        match outcome {
            Outcome::Success => SUCCESS,
            Outcome::Warnings => WARNINGS,
            Outcome::Stopped => STOPPED,
            Outcome::Failed => ERROR,
            Outcome::NotReady => VERIFY,
            Outcome::Interrupted => INTERRUPTED,
        }
    }
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
    ExitCode::from(code.unwrap_or_else(|code| code))
}

/// Where the human-readable summary goes and how to lay it out: stdout, or
/// stderr with `--json`, or nowhere with `--quiet`.
fn summary_output(json: bool, cli: &Cli) -> Option<(Box<dyn Write>, ui::SummaryFormat)> {
    if cli.quiet {
        return None;
    }
    let (sink, term): (Box<dyn Write>, _) = if json {
        (Box::new(std::io::stderr()), console::Term::stderr())
    } else {
        (Box::new(std::io::stdout()), console::Term::stdout())
    };
    let tty = term.is_term();
    let format = ui::SummaryFormat {
        style: ui::Style {
            color: ui::color_enabled(cli.no_color, tty),
        },
        width: tty
            .then(|| term.size_checked())
            .flatten()
            .map(|(_, cols)| usize::from(cols)),
        verbose: cli.verbose,
    };
    Some((sink, format))
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

/// Prints `value` as JSON when `json` is set.
fn maybe_json(json: bool, value: &impl serde::Serialize, term: &Terminal) -> Result<(), u8> {
    if json {
        print_json(value, term)
    } else {
        Ok(())
    }
}

fn detect(path: &std::path::Path, command: &str, term: &Terminal) -> Result<WorldLayout, u8> {
    layout::detect(path).map_err(|e| {
        term.error(&messages::layout(&e, command));
        exit::code(e.outcome())
    })
}

fn report_error(e: &ReportError, command: &str, term: &Terminal) -> u8 {
    term.error(&messages::report(e, command));
    exit::code(e.outcome())
}

fn finish(warned: bool, strict: bool) -> u8 {
    exit::code(Outcome::finished(warned, strict))
}

/// A Ctrl-C that arrived after the core returned still ends the run with 130,
/// before any result is printed.
fn check_interrupted(term: &Terminal) -> Result<(), u8> {
    if CANCEL.load(Ordering::SeqCst) {
        term.error(messages::INTERRUPTED);
        return Err(exit::INTERRUPTED);
    }
    Ok(())
}

/// The lines that end a finished `gc --out` or `wgc verify`: the result, the
/// checks ignored with `--force`, and how to swap the output in.
struct Finished<'a> {
    headline: String,
    forced: &'a [String],
    layout_kind: &'a str,
    source: &'a std::path::Path,
    world_folders: &'a [String],
    out: &'a std::path::Path,
}

fn print_finished(sink: &mut dyn Write, style: ui::Style, f: &Finished<'_>) {
    let _ = writeln!(sink, "{}", f.headline);
    if !f.forced.is_empty() {
        let _ = writeln!(sink, "{}", style.yellow(&messages::forced_output(f.forced)));
    }
    let _ = ui::swap_steps(sink, f.layout_kind, f.source, f.world_folders, f.out);
}

fn scan(a: &ScanArgs, cli: &Cli, term: &Terminal) -> Result<u8, u8> {
    let layout = detect(&a.path, "scan", term)?;
    let report = wgc::scan::run_scan(&FsWorld::new(&layout), &Rayon, term, &CANCEL)
        .map_err(|e| report_error(&ReportError::from(e), "scan", term))?;
    check_interrupted(term)?;
    if let Some((mut out, format)) = summary_output(a.json, cli) {
        let _ = ui::summary(&mut out, &report, &layout.root, SummaryKind::Scan, format);
    }
    maybe_json(a.json, &report, term)?;
    Ok(finish(term.warnings() > 0 || report.has_stops(), a.strict))
}

fn gc(a: &GcArgs, cli: &Cli, term: &Terminal) -> Result<u8, u8> {
    let layout = detect(&a.path, "gc", term)?;
    let gc_dims: Option<BTreeSet<_>> =
        (!a.dimensions.is_empty()).then(|| a.dimensions.iter().cloned().collect());
    if let Some(missing) = gc_dims
        .as_ref()
        .and_then(|dims| layout.unknown_dimension(dims))
    {
        term.error(&messages::unknown_dimension(missing.as_str()));
        return Err(exit::USAGE);
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
    gc_dims: Option<BTreeSet<wgc_fs::types::DimensionId>>,
) -> Result<u8, u8> {
    let world = FsWorld::new(layout);
    let report = wgc::scan::run_estimate(&world, &Rayon, gc_dims, term, &CANCEL)
        .map_err(|e| report_error(&ReportError::from(e), "gc", term))?;
    check_interrupted(term)?;
    if let Some((mut out, format)) = summary_output(a.json, cli) {
        let kind = SummaryKind::Estimate;
        let _ = ui::summary(&mut out, &report.scan, &layout.root, kind, format);
        let _ = writeln!(out, "{}", messages::NOTHING_WRITTEN);
    }
    maybe_json(a.json, &report, term)?;
    Ok(finish(
        term.warnings() > 0 || report.scan.has_stops(),
        a.strict,
    ))
}

fn gc_output(
    a: &GcArgs,
    cli: &Cli,
    term: &Terminal,
    layout: &WorldLayout,
    gc_dims: Option<BTreeSet<wgc_fs::types::DimensionId>>,
    out: &std::path::Path,
) -> Result<u8, u8> {
    let options = GcOptions {
        out: out.to_path_buf(),
        gc_dims,
        allowed: a
            .force
            .as_ref()
            .map(args::Force::allowed)
            .unwrap_or_default(),
    };
    let world = FsWorld::new(layout);
    let outcome = wgc::gc::run_gc(&world, &Rayon, &options, term, &CANCEL)
        .map_err(OutputError::from)
        .map_err(|e| {
            term.error(&messages::output(&e));
            exit::code(e.outcome())
        })?;
    let report = report::gc_output_report(&outcome).map_err(|e| report_error(&e, "gc", term))?;
    if let Some((mut sink, format)) = summary_output(a.json, cli) {
        let style = format.style;
        let kind = SummaryKind::Output;
        let _ = ui::summary(&mut sink, &report.scan, &layout.root, kind, format);
        let forced: Vec<String> = outcome.forced.iter().map(ToString::to_string).collect();
        let finished = Finished {
            headline: format!("{} {}", style.green(messages::DONE), outcome.out.display()),
            forced: &forced,
            layout_kind: layout.kind.as_str(),
            source: &layout.root,
            world_folders: &report::world_folder_names(layout),
            out: &outcome.out,
        };
        print_finished(&mut sink, style, &finished);
    }
    maybe_json(a.json, &report, term)?;
    Ok(finish(term.warnings() > 0, a.strict))
}

fn verify(a: &VerifyArgs, cli: &Cli, term: &Terminal) -> Result<u8, u8> {
    let result = output::verify(&a.out, a.source.as_deref());
    check_interrupted(term)?;
    match &result {
        Ok(ok) => {
            if let Some((mut sink, format)) = summary_output(a.json, cli) {
                let finished = Finished {
                    headline: format.style.green(messages::READY),
                    forced: &ok.forced,
                    layout_kind: &ok.layout,
                    source: &ok.source,
                    world_folders: &ok.world_folders,
                    out: &a.out,
                };
                print_finished(&mut sink, format.style, &finished);
            }
        }
        Err(e) => term.error(&messages::verify(e)),
    }
    maybe_json(a.json, &VerifyJson::new(&result), term)?;
    Ok(exit::code(
        result
            .as_ref()
            .map_or_else(VerifyError::outcome, |_| Outcome::Success),
    ))
}
