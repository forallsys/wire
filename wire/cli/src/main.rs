// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![feature(sync_nonpoison)]
#![feature(nonpoison_mutex)]

use std::process::Command;

use crate::cli::Cli;
use crate::cli::ToSubCommandModifiers;
use crate::tracing_setup::setup_logging;
use clap::CommandFactory;
use clap::Parser;
use clap_complete::generate;
use lib::hive::Hive;
use lib::hive::get_hive_location;
use miette::IntoDiagnostic;
use miette::Result;
use tracing::error;
use tracing::warn;

#[macro_use]
extern crate enum_display_derive;

mod apply;
mod cli;
mod tracing_setup;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    let args = Cli::parse();

    let modifiers = args.to_subcommand_modifiers();
    setup_logging(&args.verbose, !&args.no_progress);

    #[cfg(debug_assertions)]
    if args.markdown_help {
        clap_markdown::print_help_markdown::<Cli>();
        return Ok(());
    }

    if !matches!(args.command, cli::Commands::Completions { .. }) && !check_nix_available() {
        miette::bail!("Nix is not available on this system.");
    }

    let location = get_hive_location(args.path)?;

    match args.command {
        cli::Commands::Apply(apply_args) => {
            let mut hive = Hive::new_from_path(&location, modifiers).await?;
            apply::apply(&mut hive, location, apply_args, modifiers).await?;
        }
        cli::Commands::Inspect { json } => println!("{}", {
            let hive = Hive::new_from_path(&location, modifiers).await?;
            if json {
                serde_json::to_string(&hive).into_diagnostic()?
            } else {
                warn!("use --json to output something scripting suitable");
                format!("{hive}")
            }
        }),
        cli::Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.clone();
            generate(shell, &mut cmd, name.get_name(), &mut std::io::stdout());
        }
    }

    Ok(())
}

fn check_nix_available() -> bool {
    match Command::new("nix")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => true,
        Err(e) => {
            if let std::io::ErrorKind::NotFound = e.kind() {
                false
            } else {
                error!(
                    "Something weird happened checking for nix availability, {}",
                    e
                );
                false
            }
        }
    }
}
