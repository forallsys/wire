// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#![feature(sync_nonpoison)]
#![feature(nonpoison_mutex)]

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::cli::Cli;
use crate::cli::ToSubCommandModifiers;
use crate::sigint::handle_signals;
use crate::tracing_setup::setup_logging;
use clap::CommandFactory;
use clap::Parser;
use clap_complete::CompleteEnv;
use miette::IntoDiagnostic;
use miette::Result;
use signal_hook::consts::SIGINT;
use signal_hook_tokio::Signals;
use tracing::error;
use tracing::warn;
use wire_core::cache::InspectionCache;
use wire_core::commands::common::get_hive_node_names;
use wire_core::hive::Hive;
use wire_core::hive::get_hive_location;

#[macro_use]
extern crate enum_display_derive;

mod apply;
mod cli;
mod sigint;
mod tracing_setup;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    CompleteEnv::with_factory(Cli::command).complete();

    let args = Cli::parse();

    let modifiers = args.to_subcommand_modifiers();
    // disable progress when running inspect mode.
    setup_logging(
        &args.verbose,
        !matches!(args.command, cli::Commands::Inspect { .. }) && !&args.no_progress,
    );

    #[cfg(debug_assertions)]
    if args.markdown_help {
        clap_markdown::print_help_markdown::<Cli>();
        return Ok(());
    }

    if !check_nix_available() {
        miette::bail!("Nix is not available on this system.");
    }

    let signals = Signals::new([SIGINT]).into_diagnostic()?;
    let signals_handle = signals.handle();
    let should_shutdown = Arc::new(AtomicBool::new(false));
    let signals_task = tokio::spawn(handle_signals(signals, should_shutdown.clone()));

    let location = get_hive_location(args.path, modifiers).await?;
    let cache = InspectionCache::new().await;

    match args.command {
        cli::Commands::Apply(apply_args) => {
            let mut hive = Hive::new_from_path(&location, cache.clone(), modifiers).await?;
            apply::apply(&mut hive, should_shutdown, location, apply_args, modifiers).await?;
        }
        cli::Commands::Inspect { json, selection } => println!("{}", {
            match selection {
                cli::Inspection::Full => {
                    let hive = Hive::new_from_path(&location, cache.clone(), modifiers).await?;
                    if json {
                        serde_json::to_string(&hive).into_diagnostic()?
                    } else {
                        warn!("use --json to output something scripting suitable");
                        format!("{hive}")
                    }
                }
                cli::Inspection::Names => {
                    serde_json::to_string(&get_hive_node_names(&location, modifiers).await?)
                        .into_diagnostic()?
                }
            }
        }),
    }

    if let Some(cache) = cache {
        cache.gc().await.into_diagnostic()?;
    }

    signals_handle.close();
    signals_task.await.into_diagnostic()?;

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
