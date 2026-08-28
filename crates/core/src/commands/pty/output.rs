// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use crate::{
    commands::{
        ChildOutputMode,
        pty::{
            FAILED_PATTERN, Needles, STARTED_PATTERN, SUCCEEDED_PATTERN, SearchFindings, Status,
            logbuffer::LogBuffer,
        },
    },
    errors::CommandError,
    hive::node::BuildNameMap,
};
use aho_corasick::AhoCorasick;
use std::{
    collections::VecDeque,
    io::Write,
    sync::{Arc, Mutex},
};
use tokio::sync::{oneshot, watch};
use tracing::{Span, debug, instrument};

pub(super) struct WatchStdoutArguments {
    pub began_tx: oneshot::Sender<()>,
    pub reader: super::MasterReader,
    pub needles: Needles,
    pub output_mode: ChildOutputMode,
    pub stderr_collection: Arc<Mutex<VecDeque<String>>>,
    pub stdout_collection: Arc<Mutex<VecDeque<String>>>,
    pub status_sender: watch::Sender<Status>,
    pub span: Span,
    pub log_stdout: bool,
    pub build_name_map: BuildNameMap,
    pub print_build_logs: bool,
}

/// Handles data from the PTY, and logs or prompts the user depending on the state
/// of the command.
///
/// Emits a message on the `began_tx` when the command is considered started.
///
/// Records stderr and stdout when it is considered notable (all stdout, last few stderr messages)
#[instrument(skip_all, name = "log", parent = arguments.span)]
pub(super) fn handle_pty_stdout(arguments: WatchStdoutArguments) -> Result<(), CommandError> {
    let WatchStdoutArguments {
        began_tx,
        mut reader,
        needles,
        output_mode,
        stdout_collection,
        stderr_collection,
        status_sender,
        log_stdout,
        build_name_map,
        print_build_logs,
        ..
    } = arguments;

    let aho_corasick = AhoCorasick::builder()
        .ascii_case_insensitive(false)
        .match_kind(aho_corasick::MatchKind::LeftmostFirst)
        .build([
            needles.start.as_ref(),
            needles.succeed.as_ref(),
            needles.fail.as_ref(),
        ])
        .unwrap();

    let mut buffer = [0u8; 1024];
    let mut stderr = std::io::stderr();
    let mut began = false;
    let mut log_buffer = LogBuffer::new();
    let mut raw_mode_buffer = Vec::new();
    let mut belled = false;
    let mut began_tx = Some(began_tx);

    'outer: loop {
        match reader.read(&mut buffer) {
            Ok(0) => break 'outer,
            Ok(n) => {
                // this block is responsible for outputting the "raw" data,
                // mostly sudo prompts.
                if !began {
                    let findings = handle_rawmode_data(
                        &mut stderr,
                        &buffer,
                        n,
                        &mut raw_mode_buffer,
                        &aho_corasick,
                        &status_sender,
                        &mut began_tx,
                    )?;

                    match findings {
                        SearchFindings::Terminate => break 'outer,
                        SearchFindings::Started => {
                            began = true;
                            continue;
                        }
                        SearchFindings::None => {}
                    }

                    if belled {
                        continue;
                    }

                    stderr
                        .write(b"\x07") // bell
                        .map_err(CommandError::WritingClientStderr)?;
                    stderr.flush().map_err(CommandError::WritingClientStderr)?;

                    belled = true;

                    continue;
                }

                log_buffer.process_slice(&buffer[..n]);

                while let Some(mut line) = log_buffer.next_line() {
                    let findings =
                        search_string(&aho_corasick, &line, &status_sender, &mut began_tx);

                    match findings {
                        SearchFindings::Terminate => break 'outer,
                        SearchFindings::Started => {
                            began = true;
                            continue;
                        }
                        SearchFindings::None => {}
                    }

                    handle_normal_data(
                        &stderr_collection,
                        &stdout_collection,
                        &mut line,
                        log_stdout,
                        output_mode.clone(),
                        &build_name_map,
                        print_build_logs,
                    );
                }
            }
            Err(e) => {
                eprintln!("Error reading from PTY: {e}");
                break;
            }
        }
    }

    began_tx.map(|began_tx| began_tx.send(()));

    // failsafe if there were errors or the reader stopped
    if matches!(*status_sender.borrow(), Status::Running) {
        status_sender.send_replace(Status::Done { success: false });
    }

    debug!("stdout: goodbye");

    Ok(())
}

/// handles raw data, prints to stderr when a prompt is detected
pub(super) fn handle_rawmode_data<W: std::io::Write>(
    stderr: &mut W,
    buffer: &[u8],
    n: usize,
    raw_mode_buffer: &mut Vec<u8>,
    aho_corasick: &AhoCorasick,
    status_sender: &watch::Sender<Status>,
    began_tx: &mut Option<oneshot::Sender<()>>,
) -> Result<SearchFindings, CommandError> {
    raw_mode_buffer.extend_from_slice(&buffer[..n]);

    let findings = search_string(aho_corasick, raw_mode_buffer, status_sender, began_tx);

    if matches!(
        findings,
        SearchFindings::Started | SearchFindings::Terminate
    ) {
        return Ok(findings);
    }

    stderr
        .write_all(&buffer[..n])
        .map_err(CommandError::WritingClientStderr)?;

    stderr.flush().map_err(CommandError::WritingClientStderr)?;

    Ok(findings)
}

/// handles data when the command is considered "started", logs and records errors as appropriate
fn handle_normal_data(
    stderr_collection: &Arc<Mutex<VecDeque<String>>>,
    stdout_collection: &Arc<Mutex<VecDeque<String>>>,
    line: &mut [u8],
    log_stdout: bool,
    output_mode: ChildOutputMode,
    build_name_map: &BuildNameMap,
    print_build_logs: bool,
) {
    if line.starts_with(b"#") {
        let stripped = &mut line[1..];

        if log_stdout {
            output_mode.trace_slice(stripped, build_name_map, print_build_logs);
        }

        stdout_collection
            .lock()
            .unwrap()
            .push_front(String::from_utf8_lossy(stripped).to_string());
        return;
    }

    let log = output_mode.trace_slice(line, build_name_map, print_build_logs);

    if let Some(error_msg) = log {
        let mut queue = stderr_collection.lock().unwrap();

        // add at most 20 message to the front, drop the rest.
        queue.push_front(error_msg);
        queue.truncate(20);
    }
}

/// returns true if the command is considered stopped
fn search_string(
    aho_corasick: &AhoCorasick,
    haystack: &[u8],
    status_sender: &watch::Sender<Status>,
    began_tx: &mut Option<oneshot::Sender<()>>,
) -> SearchFindings {
    for haystack in haystack.split(|&b| b == b'\n') {
        if haystack.starts_with(b"debug") || haystack.starts_with(b"ssh") {
            tracing::log::log!(
                if haystack.starts_with(b"ssh") {
                    tracing::log::Level::Error
                } else {
                    tracing::log::Level::Debug
                },
                "{}",
                String::from_utf8_lossy(haystack)
            );

            continue;
        }

        let mut started = false;
        let mut succeeded = false;
        let mut failed = false;

        for m in aho_corasick.find_iter(haystack) {
            match m.pattern() {
                p if p == *STARTED_PATTERN => started = true,
                p if p == *SUCCEEDED_PATTERN => succeeded = true,
                p if p == *FAILED_PATTERN => failed = true,
                _ => {}
            }
        }

        if started {
            debug!("start needle was found, switching mode...");
            if let Some(began_tx) = began_tx.take() {
                let _ = began_tx.send(());
            }
        }

        if succeeded {
            debug!("succeed needle was found, marking child as succeeding.");
            status_sender.send_replace(Status::Done { success: true });
        }

        if failed {
            debug!("failed needle was found, elevated child did not succeed.");
            status_sender.send_replace(Status::Done { success: false });
        }

        if succeeded || failed {
            return SearchFindings::Terminate;
        }

        if started {
            return SearchFindings::Started;
        }
    }

    SearchFindings::None
}
