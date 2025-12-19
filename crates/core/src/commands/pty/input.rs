// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::os::fd::{AsFd, OwnedFd};

use nix::{
    poll::{PollFd, PollFlags, PollTimeout, poll},
    unistd::read,
};
use tracing::{Span, debug, error, instrument, trace};

use crate::{
    commands::pty::{MasterWriter, THREAD_BEGAN_SIGNAL, THREAD_QUIT_SIGNAL},
    errors::CommandError,
};

/// Exits on any data written to `cancel_pipe_r`
/// A pipe is used to cancel the function.
#[instrument(skip_all, level = "trace", parent = span)]
pub(super) fn watch_stdin_from_user(
    cancel_pipe_r: &OwnedFd,
    mut master_writer: MasterWriter,
    write_pipe_r: &OwnedFd,
    span: Span,
) -> Result<(), CommandError> {
    const WRITER_POSITION: usize = 0;
    const SIGNAL_POSITION: usize = 1;
    const USER_POSITION: usize = 2;

    let mut buffer = [0u8; 1024];
    let stdin = std::io::stdin();
    let mut cancel_pipe_buf = [0u8; 1];

    let user_stdin_fd = stdin.as_fd();
    let cancel_pipe_r_fd = cancel_pipe_r.as_fd();

    let mut all_fds = vec![
        PollFd::new(write_pipe_r.as_fd(), PollFlags::POLLIN),
        PollFd::new(cancel_pipe_r.as_fd(), PollFlags::POLLIN),
        PollFd::new(user_stdin_fd, PollFlags::POLLIN),
    ];

    loop {
        match poll(&mut all_fds, PollTimeout::NONE) {
            Ok(0) => {} // timeout, impossible
            Ok(_) => {
                // The user stdin pipe can be removed
                if all_fds.get(USER_POSITION).is_some()
                    && let Some(events) = all_fds[USER_POSITION].revents()
                    && events.contains(PollFlags::POLLIN)
                {
                    trace!("Got stdin from user...");
                    let n = read(user_stdin_fd, &mut buffer).map_err(CommandError::PosixPipe)?;
                    master_writer
                        .write_all(&buffer[..n])
                        .map_err(CommandError::WritingMasterStdout)?;
                    master_writer
                        .flush()
                        .map_err(CommandError::WritingMasterStdout)?;
                }

                if let Some(events) = all_fds[WRITER_POSITION].revents()
                    && events.contains(PollFlags::POLLIN)
                {
                    trace!("Got stdin from writer...");
                    let n = read(write_pipe_r, &mut buffer).map_err(CommandError::PosixPipe)?;
                    master_writer
                        .write_all(&buffer[..n])
                        .map_err(CommandError::WritingMasterStdout)?;
                    master_writer
                        .flush()
                        .map_err(CommandError::WritingMasterStdout)?;
                }

                if let Some(events) = all_fds[SIGNAL_POSITION].revents()
                    && events.contains(PollFlags::POLLIN)
                {
                    let n = read(cancel_pipe_r_fd, &mut cancel_pipe_buf)
                        .map_err(CommandError::PosixPipe)?;
                    let message = &cancel_pipe_buf[..n];

                    trace!("Got byte from signal pipe: {message:?}");

                    if message == THREAD_QUIT_SIGNAL {
                        return Ok(());
                    }

                    if message == THREAD_BEGAN_SIGNAL {
                        all_fds.remove(USER_POSITION);
                    }
                }
            }
            Err(e) => {
                error!("Poll error: {e}");
                break;
            }
        }
    }

    debug!("stdin_thread: goodbye");
    Ok(())
}
