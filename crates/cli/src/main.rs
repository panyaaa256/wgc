//! Command-line interface for `wgc` (docs/spec/cli.md).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use wgc_core::types::{DimensionId, GuardId};

/// Exit code for errors (docs/spec/cli.md "終了コード").
const EXIT_ERROR: u8 = 4;

#[derive(Debug, Parser)]
#[command(
    name = "wgc",
    version,
    about = "Delete unused chunks from Minecraft Java Edition worlds",
    long_about = "Delete chunks that no player has ever been near (InhabitedTime = 0) \
                  from a Minecraft Java Edition world.\n\n\
                  wgc never modifies the original world: it writes the result to a new \
                  folder, which you swap in yourself after checking it with `wgc verify`."
)]
struct Cli {
    /// Do not print progress or the human-readable summary (warnings and errors are still printed)
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Do not use colors
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Diagnose a world without writing anything
    #[command(
        after_help = "Examples:\n  wgc scan ./world\n  wgc scan ./server --json > report.json"
    )]
    Scan(ScanArgs),

    /// Estimate or perform the chunk GC
    ///
    /// Without --out, only an estimate is printed and nothing is written.
    #[command(after_help = "Examples:\n  \
        wgc gc ./world                  # estimate only\n  \
        wgc gc ./world -o ./world-gc    # write the result to ./world-gc\n  \
        wgc verify ./world-gc           # check before swapping")]
    Gc(GcArgs),

    /// Check that an output folder can be swapped in for the original world
    #[command(after_help = "Example:\n  wgc verify ./world-gc")]
    Verify(VerifyArgs),
}

#[derive(Debug, Args)]
struct ScanArgs {
    /// World folder (vanilla) or server root folder (Bukkit-based servers)
    path: PathBuf,

    /// Print the report as JSON on stdout (the summary goes to stderr)
    #[arg(long)]
    json: bool,

    /// Exit with code 1 when there are warnings
    #[arg(long)]
    strict: bool,
}

#[derive(Debug, Args)]
struct GcArgs {
    /// World folder (vanilla) or server root folder (Bukkit-based servers)
    path: PathBuf,

    /// Write the result to this new or empty folder. Without it, nothing is written
    #[arg(short, long, value_name = "DIR")]
    out: Option<PathBuf>,

    /// Only delete chunks in this dimension (repeatable), e.g. the_nether or minecraft:the_end
    #[arg(long = "dimension", visible_alias = "dim", value_name = "ID", value_parser = parse_dimension)]
    dimensions: Vec<DimensionId>,

    /// Continue past failed trust checks: all of them, or only the listed IDs (e.g. --force=T-3,T-5)
    #[arg(
        short,
        long,
        value_name = "IDS",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "all",
        value_parser = parse_force
    )]
    force: Option<Force>,

    /// Print the result as JSON on stdout (the summary goes to stderr)
    #[arg(long)]
    json: bool,

    /// Exit with code 1 when there are warnings
    #[arg(long)]
    strict: bool,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    /// Output folder created by `wgc gc --out`
    out: PathBuf,

    /// Compare against this world instead of the one recorded in the output
    #[arg(long, value_name = "PATH")]
    source: Option<PathBuf>,

    /// Print the result as JSON on stdout
    #[arg(long)]
    json: bool,
}

/// Which stops `--force` may continue past.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Force {
    All,
    Only(BTreeSet<GuardId>),
}

fn parse_force(value: &str) -> Result<Force, String> {
    if value == "all" {
        return Ok(Force::All);
    }
    value
        .split(',')
        .map(|id| {
            id.trim().parse::<GuardId>().map_err(|_| {
                let known: Vec<&str> = GuardId::ALL.iter().map(|g| g.as_str()).collect();
                format!(
                    "unknown check ID '{}' (expected one of {})",
                    id.trim(),
                    known.join(", ")
                )
            })
        })
        .collect::<Result<_, _>>()
        .map(Force::Only)
}

fn parse_dimension(value: &str) -> Result<DimensionId, String> {
    DimensionId::parse(value).map_err(|_| {
        format!("invalid dimension ID '{value}' (expected e.g. the_nether or minecraft:the_end)")
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let name = match cli.command {
        Command::Scan(_) => "scan",
        Command::Gc(_) => "gc",
        Command::Verify(_) => "verify",
    };
    eprintln!("error: `wgc {name}` is not implemented yet");
    ExitCode::from(EXIT_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn force_accepts_known_ids_and_removes_duplicates() {
        assert_eq!(parse_force("all"), Ok(Force::All));
        assert_eq!(
            parse_force("T-3,T-5,T-3"),
            Ok(Force::Only(BTreeSet::from([GuardId::T3, GuardId::T5])))
        );
        assert!(parse_force("T-9").is_err());
    }

    #[test]
    fn bare_force_means_all() {
        let cli = Cli::try_parse_from(["wgc", "gc", "w", "--force"]).unwrap();
        let Command::Gc(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.force, Some(Force::All));
    }

    #[test]
    fn dim_is_an_alias_of_dimension() {
        let cli = Cli::try_parse_from(["wgc", "gc", "w", "--dim", "the_end", "--dimension", "x:y"])
            .unwrap();
        let Command::Gc(args) = cli.command else {
            panic!()
        };
        assert_eq!(
            args.dimensions,
            [DimensionId::the_end(), DimensionId::parse("x:y").unwrap()]
        );
        assert!(Cli::try_parse_from(["wgc", "gc", "w", "--dim", "nether"]).is_err());
    }
}
