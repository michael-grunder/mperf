mod cli;
mod profile;
mod record;
mod trace;
mod view;

use anyhow::{Result, bail};
use clap::Parser;
use cli::{Cli, Command};
use std::{
    io::{self, IsTerminal},
    process::ExitCode,
};

fn main() -> ExitCode {
    let cli = Cli::parse_from(cli::arguments());
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
            {
                return ExitCode::SUCCESS;
            }
            eprintln!("mperf: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Record(args) => record::record(&args),
        Command::List { kind } => trace::list(kind),
        Command::Report(args) => {
            if args.view.sort == Some(cli::Sort::Delta) {
                bail!("--sort delta is only available in mperf diff; see 'mperf report help'");
            }
            let path = args.input.map_or_else(record::latest, Ok)?;
            let profile = trace::load(&path, &args.selection)?;
            present(&profile, None, &args.view)
        }
        Command::Diff(args) => {
            let new_path = args.new.map_or_else(record::latest, Ok)?;
            let old_path = args.baseline.unwrap_or_else(|| record::old_path(&new_path));
            let old = trace::load(&old_path, &args.selection)?;
            let new = trace::load(&new_path, &args.selection)?;
            if old.unit != new.unit {
                bail!(
                    "incompatible weight units: baseline '{}' vs new '{}'; select matching CPU schemas",
                    old.unit,
                    new.unit
                );
            }
            present(&new, Some(&old), &args.view)
        }
    }
}

fn present(
    profile: &profile::Profile,
    baseline: Option<&profile::Profile>,
    options: &cli::ViewOptions,
) -> Result<()> {
    let terminal = io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var("TERM").is_ok_and(|t| t != "dumb");
    if options.tui && !terminal {
        bail!(
            "--tui requires terminal input/output and TERM set to a capable terminal; use --stdio"
        );
    }
    if options.stdio || !terminal {
        view::print(profile, baseline, options)
    } else {
        view::interactive(profile, baseline, options)
    }
}
