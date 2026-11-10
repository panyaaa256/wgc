//! Terminal output: colors, progress, warnings and the human-readable summary
//! (docs/spec/scan.md "ターミナル出力", docs/spec/cli.md "出力").

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use wgc_core::layout::{LayoutKind, WorldLayout};
use wgc_core::report::{DimensionReport, LockStatus, ScanReport, TrustStatus};
use wgc_core::types::{GuardId, Observer, Progress, Warning};

use crate::messages;

/// Whether to use ANSI colors on a stream (docs/spec/cli.md "出力").
pub fn color_enabled(no_color_flag: bool, is_terminal: bool) -> bool {
    color_allowed(
        no_color_flag,
        is_terminal,
        std::env::var_os("NO_COLOR").as_deref(),
        std::env::var_os("TERM").as_deref(),
    )
}

/// The rule behind [`color_enabled`], with the environment passed in.
fn color_allowed(
    no_color_flag: bool,
    is_terminal: bool,
    no_color_env: Option<&std::ffi::OsStr>,
    term: Option<&std::ffi::OsStr>,
) -> bool {
    let no_color = no_color_env.is_some_and(|v| !v.is_empty());
    let dumb = term.is_some_and(|v| v == "dumb");
    is_terminal && !no_color && !dumb && !no_color_flag
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
    pub fn bold(self, t: &str) -> String {
        self.paint("1", t)
    }
    pub fn red(self, t: &str) -> String {
        self.paint("1;31", t)
    }
    pub fn yellow(self, t: &str) -> String {
        self.paint("1;33", t)
    }
    pub fn green(self, t: &str) -> String {
        self.paint("32", t)
    }
    pub fn bold_green(self, t: &str) -> String {
        self.paint("1;32", t)
    }
    pub fn cyan(self, t: &str) -> String {
        self.paint("36", t)
    }
    /// For secondary text that should step back from the rest.
    pub fn dim(self, t: &str) -> String {
        self.paint("2", t)
    }
}

const PROGRESS_TEMPLATE: &str = "{prefix:<9} [{bar:20}] {pos}/{len}  ETA {eta}";
const PROGRESS_TEMPLATE_COLOR: &str = "{prefix:<9} [{bar:20.cyan/blue}] {pos}/{len}  ETA {eta}";

fn progress_style(color: bool) -> ProgressStyle {
    let template = if color {
        PROGRESS_TEMPLATE_COLOR
    } else {
        PROGRESS_TEMPLATE
    };
    ProgressStyle::with_template(template)
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("#>-")
}

/// The bar of the phase in progress, labeled with the phase name.
type ActiveBar = Option<(&'static str, ProgressBar)>;

/// Prints warnings (always) and progress (only on a terminal, unless quiet) to stderr.
pub struct Terminal {
    pub style: Style,
    show_progress: bool,
    warnings: AtomicU64,
    bar: Mutex<ActiveBar>,
}

impl Terminal {
    pub fn new(quiet: bool, no_color: bool) -> Self {
        let tty = std::io::stderr().is_terminal();
        Terminal {
            style: Style {
                color: color_enabled(no_color, tty),
            },
            show_progress: tty && !quiet,
            warnings: AtomicU64::new(0),
            bar: Mutex::new(None),
        }
    }

    pub fn warnings(&self) -> u64 {
        self.warnings.load(Ordering::Relaxed)
    }

    /// Clears the bar for good: nothing is drawn after an error.
    pub fn error(&self, text: &str) {
        if let Some((_, bar)) = self.lock_bar().take() {
            bar.finish_and_clear();
        }
        eprintln!("{} {text}", self.style.red("error:"));
    }

    /// Hides the bar while printing, then redraws it below the warning.
    pub fn warn_text(&self, text: &str) {
        self.warnings.fetch_add(1, Ordering::Relaxed);
        let line = format!("{} {text}", self.style.yellow("warning:"));
        match &*self.lock_bar() {
            Some((_, bar)) => bar.suspend(|| eprintln!("{line}")),
            None => eprintln!("{line}"),
        }
    }

    fn lock_bar(&self) -> MutexGuard<'_, ActiveBar> {
        self.bar.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Observer for Terminal {
    fn progress(&self, p: Progress) {
        if !self.show_progress {
            return;
        }
        let (label, done, total) = match p {
            Progress::Reading { done, total } => ("Reading", done, total),
            Progress::Writing { done, total } => ("Writing", done, total),
            Progress::Verifying { done, total } => ("Verifying", done, total),
        };
        let mut active = self.lock_bar();
        // Core reports the phases one after another, each starting from `done = 0`.
        if active.as_ref().is_none_or(|(l, _)| *l != label) {
            let bar = ProgressBar::with_draw_target(Some(total), ProgressDrawTarget::stderr())
                .with_style(progress_style(self.style.color))
                .with_prefix(label);
            *active = Some((label, bar));
        }
        if let Some((_, bar)) = active.as_ref() {
            bar.set_position(done);
            if done >= total {
                bar.finish();
                *active = None;
            }
        }
    }

    fn warn(&self, w: Warning) {
        self.warn_text(&messages::warning(&w));
    }
}

/// `1.5 GiB` style sizes.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

const LIST_LIMIT: usize = 10;

fn list<T>(out: &mut dyn Write, items: &[T], show: impl Fn(&T) -> String) -> std::io::Result<()> {
    for item in items.iter().take(LIST_LIMIT) {
        writeln!(out, "    {}", show(item))?;
    }
    if items.len() > LIST_LIMIT {
        writeln!(
            out,
            "    ... and {} more (see --json)",
            items.len() - LIST_LIMIT
        )?;
    }
    Ok(())
}

/// Which sections to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryKind {
    Scan,
    Estimate,
    Output,
}

/// How to lay out the summary for the stream it goes to.
#[derive(Debug, Clone, Copy)]
pub struct SummaryFormat {
    pub style: Style,
    /// Columns of the destination terminal; `None` when it is not a terminal.
    pub width: Option<usize>,
    pub verbose: bool,
}

/// Counts of the per-dimension lists that only `--verbose` prints.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ListCounts {
    pub integrity: u64,
    pub empty_files: u64,
    pub orphans: u64,
    pub seams: u64,
}

impl ListCounts {
    fn of(d: &DimensionReport) -> Self {
        ListCounts {
            integrity: (d.integrity.structural.len() + d.integrity.content.len()) as u64,
            empty_files: d.empty_files.len() as u64,
            orphans: d.orphans.entities + d.orphans.poi,
            seams: d.boundary_warnings.len() as u64,
        }
    }

    fn add(self, o: Self) -> Self {
        ListCounts {
            integrity: self.integrity + o.integrity,
            empty_files: self.empty_files + o.empty_files,
            orphans: self.orphans + o.orphans,
            seams: self.seams + o.seams,
        }
    }

    fn any(self) -> bool {
        self != Self::default()
    }
}

/// One row of the dimension table; the total is a row too.
#[derive(Debug, Default, Clone, Copy)]
struct DimensionRow<'a> {
    label: &'a str,
    before: u64,
    chunks: u64,
    delete: u64,
    after: u64,
    counts: ListCounts,
}

impl<'a> DimensionRow<'a> {
    fn of(d: &'a DimensionReport) -> Self {
        DimensionRow {
            label: d.id.as_str(),
            before: d.size.region + d.size.entities + d.size.poi,
            chunks: d.chunks.total,
            delete: d.gc.delete,
            after: d.gc.size_after.region + d.gc.size_after.entities + d.gc.size_after.poi,
            counts: ListCounts::of(d),
        }
    }

    fn total(rows: &[Self]) -> Self {
        rows.iter().fold(
            DimensionRow {
                label: "total",
                ..Default::default()
            },
            |t, r| DimensionRow {
                before: t.before + r.before,
                chunks: t.chunks + r.chunks,
                delete: t.delete + r.delete,
                after: t.after + r.after,
                counts: t.counts.add(r.counts),
                ..t
            },
        )
    }

    fn wide_cells(&self) -> [String; 8] {
        [
            bytes(self.before),
            self.chunks.to_string(),
            self.delete.to_string(),
            bytes(self.after),
            self.counts.integrity.to_string(),
            self.counts.empty_files.to_string(),
            self.counts.orphans.to_string(),
            self.counts.seams.to_string(),
        ]
    }
}

const DIMENSIONS_HEADER: &str = "Dimensions:";
const WIDE_COLUMNS: [&str; 8] = [
    "size", "chunks", "delete", "after", "problems", "empty", "orphans", "seams",
];

fn text_width(s: &str) -> usize {
    s.chars().count()
}

/// The wide dimension table and its width without color codes.
fn wide_table(
    rows: &[DimensionRow<'_>],
    total: &DimensionRow<'_>,
    style: Style,
) -> (usize, Vec<String>) {
    let body: Vec<_> = rows
        .iter()
        .chain([total])
        .map(|r| (r.label, r.wide_cells()))
        .collect();
    // Rows are indented by two spaces under the header.
    let label_width = body
        .iter()
        .map(|(label, _)| text_width(label))
        .max()
        .unwrap_or(0)
        .max(text_width(DIMENSIONS_HEADER) - 2);
    let mut widths = WIDE_COLUMNS.map(text_width);
    for (_, cells) in &body {
        for (w, cell) in widths.iter_mut().zip(cells) {
            *w = (*w).max(text_width(cell));
        }
    }
    // Cells are padded before they are painted, so color codes never count towards the width.
    // Returns the cells without and with color.
    let columns = |cells: &mut dyn Iterator<Item = &str>| -> (String, String) {
        cells.zip(widths).fold(
            (String::new(), String::new()),
            |(mut plain, mut painted), (cell, w)| {
                let padded = format!("{cell:>w$}");
                plain.push_str("  ");
                plain.push_str(&padded);
                painted.push_str("  ");
                painted.push_str(&if cell == "0" {
                    style.dim(&padded)
                } else {
                    padded
                });
                (plain, painted)
            },
        )
    };

    let (header_columns, _) = columns(&mut WIDE_COLUMNS.into_iter());
    let header = format!(
        "{DIMENSIONS_HEADER:<w$}{header_columns}",
        w = label_width + 2
    );
    let mut width = text_width(&header);
    let mut lines = vec![style.bold(&header)];
    for (label, cells) in &body {
        let label = format!("{label:<label_width$}");
        let (mut plain, mut painted) = columns(&mut cells.iter().map(String::as_str));
        let is_total = lines.len() == body.len();
        if is_total {
            let saves = saves(total);
            plain.push_str(&format!("  ({saves})"));
            painted.push_str(&format!("  ({})", style.bold_green(&saves)));
        }
        width = width.max(2 + text_width(&label) + text_width(&plain));
        let label = if is_total { style.bold(&label) } else { label };
        lines.push(format!("  {label}{painted}"));
    }
    (width, lines)
}

fn saves(total: &DimensionRow<'_>) -> String {
    format!("saves {}", bytes(total.before.saturating_sub(total.after)))
}

fn narrow_table(
    out: &mut dyn Write,
    rows: &[DimensionRow<'_>],
    total: &DimensionRow<'_>,
    style: Style,
) -> std::io::Result<()> {
    let w = rows
        .iter()
        .chain([total])
        .map(|r| text_width(r.label))
        .max()
        .unwrap_or(0);
    writeln!(out, "{}", style.bold(DIMENSIONS_HEADER))?;
    for r in rows {
        writeln!(
            out,
            "  {:<w$} {:>10}  {:>8} chunks  {:>8} to delete  -> {:>10}",
            r.label,
            bytes(r.before),
            r.chunks,
            r.delete,
            bytes(r.after)
        )?;
    }
    writeln!(
        out,
        "  {} {:>10}  -> {} ({})",
        style.bold(&format!("{:<w$}", total.label)),
        bytes(total.before),
        bytes(total.after),
        style.bold_green(&saves(total))
    )
}

/// The human-readable summary of a report.
pub fn summary(
    out: &mut dyn Write,
    r: &ScanReport,
    root: &Path,
    kind: SummaryKind,
    format: SummaryFormat,
) -> std::io::Result<()> {
    let style = format.style;
    let mut details_hidden = false;
    // 1. Layout, version, lock.
    writeln!(
        out,
        "{} {} ({}), data version {}",
        style.bold("World:"),
        style.cyan(&root.display().to_string()),
        r.layout.kind,
        r.world_data_version
            .map_or("unknown".to_owned(), |v| v.to_string())
    )?;
    let lock = match r.lock {
        LockStatus::Acquired => style.green("locked"),
        LockStatus::InUse => style.yellow("in use (results may be inaccurate)"),
        LockStatus::Missing => style.yellow("session.lock missing"),
        LockStatus::NotWritable => style.yellow("session.lock not writable"),
        LockStatus::Error => style.yellow("could not lock"),
    };
    writeln!(out, "{} {lock}", style.bold("Lock:"))?;

    // 2. Trust checks.
    writeln!(out, "{}", style.bold("Checks:"))?;
    let mut stopping = Vec::new();
    for item in &r.trust {
        let status = match item.status {
            TrustStatus::Pass => style.green("pass"),
            TrustStatus::NotApplicable => "n/a".to_owned(),
            TrustStatus::Fail => {
                stopping.push(item.id.as_str());
                style.red("FAIL")
            }
            TrustStatus::Unknown => {
                stopping.push(item.id.as_str());
                style.yellow("UNKNOWN")
            }
        };
        let row = format!("{} {:<36} {status}", item.id, messages::check_name(item.id));
        let row = if item.status == TrustStatus::NotApplicable {
            style.dim(&row)
        } else {
            row
        };
        writeln!(out, "  {row}")?;
        if format.verbose {
            list(out, &item.details, messages::detail)?;
        } else {
            details_hidden |= !item.details.is_empty();
        }
        if let Some(reason) = &item.reason {
            writeln!(out, "    {}", style.dim(&messages::reason(reason)))?;
        }
    }
    if r.lock == LockStatus::Missing {
        stopping.push(GuardId::L1.as_str());
    }
    if !stopping.is_empty() && kind != SummaryKind::Output {
        writeln!(
            out,
            "  `wgc gc --out` stops on {}. Continue only if you are sure: --force={}",
            stopping.join(", "),
            stopping.join(",")
        )?;
    }

    // 3-4. Per dimension and totals (docs/spec/scan.md "ディメンションの表").
    let rows: Vec<_> = r.dimensions.iter().map(DimensionRow::of).collect();
    let total = DimensionRow::total(&rows);
    let temp_files = r.other_files.omitted.len() as u64;
    let (wide_width, wide_lines) = wide_table(&rows, &total, style);
    if format.width.is_some_and(|w| wide_width <= w) {
        for line in wide_lines {
            writeln!(out, "{line}")?;
        }
        if !format.verbose && temp_files > 0 {
            writeln!(out, "{}", messages::temp_files_not_copied(temp_files))?;
        }
    } else {
        narrow_table(out, &rows, &total, style)?;
        if !format.verbose
            && let Some(line) = messages::also_found(&total.counts, temp_files)
        {
            writeln!(out, "{line}")?;
        }
    }
    if !format.verbose && (details_hidden || total.counts.any() || temp_files > 0) {
        writeln!(out, "{}", style.dim(messages::VERBOSE_HINT))?;
    }

    if format.verbose {
        lists(out, r, style)?;
    }

    // 7. Older chunks (scan only).
    let older: u64 = r
        .dimensions
        .iter()
        .map(|d| d.chunks.older_data_version)
        .sum();
    if kind == SummaryKind::Scan && older > 0 {
        writeln!(out, "{}", messages::older_chunks(older))?;
    }

    // 9. Notices.
    for n in &r.notices {
        if *n != wgc_core::report::Notice::RunOfficialOptimize {
            writeln!(out, "{} {}", style.bold("Note:"), messages::notice(*n))?;
        }
    }
    Ok(())
}

/// Sections 5, 6 and 8: the lists only `--verbose` prints.
fn lists(out: &mut dyn Write, r: &ScanReport, style: Style) -> std::io::Result<()> {
    // 5. Integrity.
    let structural: Vec<_> = r
        .dimensions
        .iter()
        .flat_map(|d| &d.integrity.structural)
        .collect();
    let content: Vec<_> = r
        .dimensions
        .iter()
        .flat_map(|d| d.integrity.content.iter().map(move |c| (d, c)))
        .collect();
    if !structural.is_empty() || !content.is_empty() {
        writeln!(
            out,
            "{}",
            style.bold("Integrity problems (these files are copied unchanged):")
        )?;
        list(out, &structural, |s| match s.index {
            Some(i) => format!("{} {}: chunk {i}", s.id.as_str(), s.path),
            None => format!("{} {}", s.id.as_str(), s.path),
        })?;
        list(out, &content, |(d, c)| {
            format!("{} chunk [{}, {}]: {}", d.id, c.pos[0], c.pos[1], c.detail)
        })?;
    }

    // 6. Clean targets.
    let empty: Vec<&String> = r.dimensions.iter().flat_map(|d| &d.empty_files).collect();
    let orphans: u64 = r
        .dimensions
        .iter()
        .map(|d| d.orphans.entities + d.orphans.poi)
        .sum();
    if !empty.is_empty() || !r.other_files.omitted.is_empty() || orphans > 0 {
        writeln!(out, "{}", style.bold("Not copied (clean rules):"))?;
        list(out, &empty, |p| format!("empty file {p}"))?;
        list(out, &r.other_files.omitted, |o| {
            format!("leftover temp file {}", o.path)
        })?;
        if orphans > 0 {
            writeln!(out, "    {orphans} entity/POI entries without a chunk")?;
        }
    }

    // 8. Boundary with old terrain.
    let clusters: Vec<_> = r
        .dimensions
        .iter()
        .flat_map(|d| d.boundary_warnings.iter().map(move |b| (d, b)))
        .collect();
    if !clusters.is_empty() {
        writeln!(
            out,
            "{}",
            style.bold("Old terrain next to deleted chunks (a visible seam may appear):")
        )?;
        list(out, &clusters, |(d, b)| {
            format!(
                "{}: {} chunks from [{}, {}] to [{}, {}]",
                d.id, b.chunks, b.min[0], b.min[1], b.max[0], b.max[1]
            )
        })?;
    }

    Ok(())
}

/// The swap steps of docs/spec/output.md "書き戻しの手順", with real paths.
pub fn swap_steps(
    out: &mut dyn Write,
    layout_kind: &str,
    source: &Path,
    world_folders: &[String],
    output: &Path,
) -> std::io::Result<()> {
    writeln!(out, "To swap in the new world:")?;
    writeln!(out, "  1. Stop the server or game.")?;
    writeln!(
        out,
        "  2. Run `wgc verify {}` and check that it says \"{}\".",
        output.display(),
        messages::READY
    )?;
    let bukkit = layout_kind == LayoutKind::Bukkit.as_str()
        || layout_kind == LayoutKind::BukkitLegacy.as_str();
    if bukkit {
        writeln!(out, "  3. Rename each world folder:")?;
        for f in world_folders {
            let src = source.join(f);
            writeln!(out, "       {} -> {}.wgc-bak", src.display(), src.display())?;
        }
        writeln!(out, "  4. Move each new world folder into place:")?;
        for f in world_folders {
            writeln!(
                out,
                "       {} -> {}",
                output.join(f).display(),
                source.join(f).display()
            )?;
        }
        writeln!(out, "     Do not move {}.", output.join(".wgc").display())?;
    } else {
        writeln!(
            out,
            "  3. Rename {} to {}.wgc-bak.",
            source.display(),
            source.display()
        )?;
        writeln!(
            out,
            "  4. Move {} to {}.",
            output.display(),
            source.display()
        )?;
    }
    writeln!(out, "  5. Start the game and check the world.")?;
    writeln!(
        out,
        "  6. Only when everything is fine, delete the .wgc-bak folder(s). Without them the old world cannot be restored."
    )?;
    if !bukkit {
        writeln!(
            out,
            "  7. You may delete {} afterwards.",
            source.join(".wgc").display()
        )?;
    }
    Ok(())
}

/// Paths for [`swap_steps`] from a layout.
pub fn layout_paths(layout: &WorldLayout) -> Vec<String> {
    layout
        .world_folders
        .iter()
        .map(|f| {
            wgc_core::report::rel_path(&layout.root, f).unwrap_or_else(|_| f.display().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    fn row(label: &str, before: u64, seams: u64) -> DimensionRow<'_> {
        DimensionRow {
            label,
            before,
            chunks: 10,
            delete: 4,
            after: before / 2,
            counts: ListCounts {
                seams,
                ..Default::default()
            },
        }
    }

    #[test]
    fn wide_table_aligns_columns_and_sums_the_total() {
        let rows = [
            row("minecraft:overworld", 4096, 2),
            row("minecraft:the_end", 0, 0),
        ];
        let total = DimensionRow::total(&rows);
        assert_eq!((total.chunks, total.counts.seams), (20, 2));

        let (width, lines) = wide_table(&rows, &total, Style { color: false });
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("Dimensions:"));
        assert!(lines[3].trim_start().starts_with("total"));
        assert!(lines[3].ends_with("(saves 2.0 KiB)"), "{}", lines[3]);
        assert_eq!(width, lines.iter().map(|l| text_width(l)).max().unwrap());
        // Every column ends at the same place in the header and the rows.
        let seams_end = lines[0].find("seams").unwrap() + "seams".len();
        assert_eq!(&lines[1][seams_end - 1..seams_end], "2");
        assert_eq!(&lines[2][seams_end - 1..seams_end], "0");
    }

    #[test]
    fn colors_do_not_change_the_measured_width() {
        let rows = [row("minecraft:overworld", 4096, 0)];
        let total = DimensionRow::total(&rows);
        let (plain, _) = wide_table(&rows, &total, Style { color: false });
        let (colored, _) = wide_table(&rows, &total, Style { color: true });
        assert_eq!(plain, colored);
    }

    #[test]
    fn progress_templates_parse() {
        for template in [PROGRESS_TEMPLATE, PROGRESS_TEMPLATE_COLOR] {
            assert!(
                ProgressStyle::with_template(template).is_ok(),
                "invalid template: {template}"
            );
        }
    }

    #[test]
    fn color_rules() {
        use std::ffi::OsStr;
        let xterm = Some(OsStr::new("xterm"));
        assert!(color_allowed(false, true, None, xterm));
        assert!(
            !color_allowed(false, false, None, xterm),
            "needs a terminal"
        );
        assert!(!color_allowed(true, true, None, xterm), "--no-color");
        assert!(
            !color_allowed(false, true, Some(OsStr::new("1")), xterm),
            "NO_COLOR"
        );
        assert!(
            color_allowed(false, true, Some(OsStr::new("")), xterm),
            "empty NO_COLOR is ignored"
        );
        assert!(
            !color_allowed(false, true, None, Some(OsStr::new("dumb"))),
            "TERM=dumb"
        );
    }

    #[test]
    fn bukkit_swap_steps_are_numbered() {
        let mut out = Vec::new();
        swap_steps(
            &mut out,
            "bukkit_legacy",
            Path::new("/srv"),
            &["world".to_owned(), "world_nether".to_owned()],
            Path::new("/out"),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        for n in 1..=6 {
            assert!(
                text.contains(&format!("  {n}. ")),
                "step {n} missing:\n{text}"
            );
        }
    }
}
