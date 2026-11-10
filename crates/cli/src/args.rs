//! Command-line arguments (docs/spec/cli.md "コマンド").

use std::collections::BTreeSet;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use wgc_core::types::{DimensionId, GuardId};

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
pub struct Cli {
    /// Do not print progress or the human-readable summary (warnings and errors are still printed)
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Do not use colors
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
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
pub struct ScanArgs {
    /// World folder (vanilla) or server root folder (Bukkit-based servers)
    pub path: PathBuf,

    /// Print the report as JSON on stdout (the summary goes to stderr)
    #[arg(long)]
    pub json: bool,

    /// Exit with code 1 when there are warnings
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, Args)]
pub struct GcArgs {
    /// World folder (vanilla) or server root folder (Bukkit-based servers)
    pub path: PathBuf,

    /// Write the result to this new or empty folder. Without it, nothing is written
    #[arg(short, long, value_name = "DIR")]
    pub out: Option<PathBuf>,

    /// Only delete chunks in this dimension (repeatable), e.g. the_nether or minecraft:the_end
    #[arg(long = "dimension", visible_alias = "dim", value_name = "ID", value_parser = parse_dimension)]
    pub dimensions: Vec<DimensionId>,

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
    pub force: Option<Force>,

    /// Print the result as JSON on stdout (the summary goes to stderr)
    #[arg(long)]
    pub json: bool,

    /// Exit with code 1 when there are warnings
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Output folder created by `wgc gc --out`
    pub out: PathBuf,

    /// Compare against this world instead of the one recorded in the output
    #[arg(long, value_name = "PATH")]
    pub source: Option<PathBuf>,

    /// Print the result as JSON on stdout
    #[arg(long)]
    pub json: bool,
}

/// Which stops `--force` may continue past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Force {
    All,
    Only(BTreeSet<GuardId>),
}

impl Force {
    /// The IDs this `--force` allows.
    pub fn allowed(&self) -> BTreeSet<GuardId> {
        match self {
            Force::All => GuardId::ALL.into_iter().collect(),
            Force::Only(ids) => ids.clone(),
        }
    }
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
