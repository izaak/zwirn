use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use zwirn::engine::{Error, Mode, Report, Request, execute};
use zwirn::fragment::FragmentPath;
use zwirn::reconcile::Operation;

#[cfg(target_os = "macos")]
mod live;

/// Synchronize external source fragments with Audulus documents.
#[derive(Debug, Parser)]
#[command(
    version,
    about,
    override_usage = "zwirn <COMMAND>",
    after_help = "DOCUMENT is an .audulus4 file.\nFor one-shot commands, FRAGMENT is an exact marker path relative to the source root; omitting fragments selects every fragment.\n\nExamples:\n  zwirn status patch.audulus4\n  zwirn sync patch.audulus4\n  zwirn embed patch.audulus4 src/filter.lua\n  zwirn live patch.audulus4"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Report the synchronization state of each selected fragment.
    Status(CommonArgs),

    /// Copy safe filesystem changes into the document.
    Embed(DirectionArgs),

    /// Copy safe document changes to the filesystem.
    Extract(DirectionArgs),

    /// Apply safe changes in both directions.
    Sync(CommonArgs),

    /// Reconcile saved changes in the foreground (macOS only).
    Live(LiveArgs),
}

#[derive(Debug, Args)]
struct CommonArgs {
    /// Root directory for fragment paths (defaults to the document's parent).
    #[arg(long, value_name = "DIR")]
    source_root: Option<PathBuf>,

    /// Audulus document to inspect or update.
    #[arg(value_name = "DOCUMENT")]
    document: PathBuf,

    /// Exact fragment paths to select (defaults to every fragment).
    #[arg(value_name = "FRAGMENT")]
    selectors: Vec<FragmentPath>,
}

#[derive(Debug, Args)]
struct DirectionArgs {
    /// Resolve selected conflicts in this direction.
    #[arg(long)]
    force: bool,

    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Debug, Args)]
struct LiveArgs {
    /// Root directory for fragment paths (defaults to the document's parent).
    #[arg(long, value_name = "DIR")]
    source_root: Option<PathBuf>,

    /// Audulus document to monitor and update.
    #[arg(value_name = "DOCUMENT")]
    document: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            return fail(format_args!(
                "cannot determine the current directory: {error}"
            ));
        }
    };

    match &cli.command {
        Command::Status(common) => execute_one_shot(&cwd, common, Mode::Status),
        Command::Embed(args) => execute_one_shot(
            &cwd,
            &args.common,
            Mode::Mutate(Operation::Embed { force: args.force }),
        ),
        Command::Extract(args) => execute_one_shot(
            &cwd,
            &args.common,
            Mode::Mutate(Operation::Extract { force: args.force }),
        ),
        Command::Sync(common) => execute_one_shot(&cwd, common, Mode::Mutate(Operation::Sync)),
        Command::Live(args) => execute_live(&cwd, args),
    }
}

fn execute_one_shot(cwd: &Path, common: &CommonArgs, mode: Mode) -> ExitCode {
    let report = match execute(Request {
        cwd,
        document: &common.document,
        source_root: common.source_root.as_deref(),
        selectors: &common.selectors,
        mode,
    }) {
        Ok(report) => report,
        Err(error) => return fail_engine(error),
    };
    finish(report)
}

#[cfg(target_os = "macos")]
fn execute_live(cwd: &Path, args: &LiveArgs) -> ExitCode {
    match live::run(cwd, &args.document, args.source_root.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(format_args!("{error}")),
    }
}

#[cfg(not(target_os = "macos"))]
fn execute_live(_cwd: &Path, _args: &LiveArgs) -> ExitCode {
    fail(format_args!(
        "the `live` command is unsupported on this platform"
    ))
}

fn finish(report: Report) -> ExitCode {
    let mut stdout = BufWriter::new(io::stdout().lock());
    let written = report
        .entries
        .iter()
        .try_for_each(|entry| writeln!(stdout, "{}\t{}", entry.path(), entry.result()));
    if let Err(error) = written.and_then(|()| stdout.flush()) {
        return fail(format_args!("cannot write command output: {error}"));
    }
    ExitCode::from(report.exit.code())
}

fn fail(message: std::fmt::Arguments<'_>) -> ExitCode {
    let _ = writeln!(io::stderr().lock(), "zwirn: {message}");
    ExitCode::from(2)
}

fn fail_engine(error: Error) -> ExitCode {
    let mut stderr = io::stderr().lock();
    for path in error.committed_external() {
        let _ = writeln!(stderr, "zwirn: external file already written for `{path}`");
    }
    match &error {
        Error::Commit(error) => {
            let _ = writeln!(stderr, "zwirn: {}", error.failure());
        }
        Error::Coordination(error) => {
            let _ = writeln!(stderr, "zwirn: {}", error.failure());
        }
        _ => {
            let _ = writeln!(stderr, "zwirn: {error}");
        }
    }
    ExitCode::from(2)
}
