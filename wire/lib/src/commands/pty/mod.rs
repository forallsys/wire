// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use crate::commands::pty::output::{WatchStdoutArguments, handle_pty_stdout};
use crate::status::STATUS;
use aho_corasick::PatternID;
use itertools::Itertools;
use nix::sys::termios::{LocalFlags, SetArg, Termios, tcgetattr, tcsetattr};
use nix::unistd::pipe;
use nix::unistd::write as posix_write;
use portable_pty::{CommandBuilder, NativePtySystem, PtyPair, PtySize};
use rand::distr::Alphabetic;
use std::collections::VecDeque;
use std::io::stderr;
use std::sync::{LazyLock, Mutex};
use std::{
    io::{Read, Write},
    os::fd::{AsFd, OwnedFd},
    sync::Arc,
};
use tokio::sync::{oneshot, watch};
use tracing::instrument;
use tracing::{Span, debug, trace};

use crate::commands::CommandArguments;
use crate::commands::pty::input::watch_stdin_from_user;
use crate::errors::CommandError;
use crate::{SubCommandModifiers, aquire_stdin_lock};
use crate::{
    commands::{ChildOutputMode, WireCommandChip},
    errors::HiveLibError,
    hive::node::Target,
};

mod input;
mod logbuffer;
mod output;

type MasterWriter = Box<dyn Write + Send>;
type MasterReader = Box<dyn Read + Send>;

/// the underlying command began
const THREAD_BEGAN_SIGNAL: &[u8; 1] = b"b";
const THREAD_QUIT_SIGNAL: &[u8; 1] = b"q";

type Child = Box<dyn portable_pty::Child + Send + Sync>;

pub(crate) struct InteractiveChildChip {
    child: Child,

    cancel_stdin_pipe_w: OwnedFd,
    write_stdin_pipe_w: OwnedFd,

    stderr_collection: Arc<Mutex<VecDeque<String>>>,
    stdout_collection: Arc<Mutex<VecDeque<String>>>,

    original_command: String,

    status_receiver: watch::Receiver<Status>,
    stdout_handle: tokio::task::JoinHandle<Result<(), CommandError>>,
}

/// sets and reverts terminal options (the terminal user interaction is performed)
/// reverts data when dropped
struct StdinTermiosAttrGuard(Termios);

#[derive(Debug)]
enum Status {
    Running,
    Done { success: bool },
}

#[derive(Debug)]
enum SearchFindings {
    None,
    Started,
    Terminate,
}

static STARTED_PATTERN: LazyLock<PatternID> = LazyLock::new(|| PatternID::must(0));
static SUCCEEDED_PATTERN: LazyLock<PatternID> = LazyLock::new(|| PatternID::must(1));
static FAILED_PATTERN: LazyLock<PatternID> = LazyLock::new(|| PatternID::must(2));

/// substitutes STDOUT with #$line. stdout is far less common than stderr.
const IO_SUBS: &str = "1> >(while IFS= read -r line; do echo \"#$line\"; done)";

fn create_ending_segment<S: AsRef<str>>(
    arguments: &CommandArguments<'_, S>,
    needles: &Needles,
) -> String {
    let Needles {
        succeed,
        fail,
        start,
    } = needles;

    format!(
        "echo -e '{succeed}' || echo '{failed}'",
        succeed = if matches!(arguments.output_mode, ChildOutputMode::Interactive) {
            format!(
                "{start}\\n{succeed}",
                start = String::from_utf8_lossy(start),
                succeed = String::from_utf8_lossy(succeed)
            )
        } else {
            String::from_utf8_lossy(succeed).to_string()
        },
        failed = String::from_utf8_lossy(fail)
    )
}

fn create_starting_segment<S: AsRef<str>>(
    arguments: &CommandArguments<'_, S>,
    start_needle: &Arc<Vec<u8>>,
) -> String {
    if matches!(arguments.output_mode, ChildOutputMode::Interactive) {
        String::new()
    } else {
        format!(
            "echo '{start}' && ",
            start = String::from_utf8_lossy(start_needle)
        )
    }
}

#[instrument(skip_all, name = "run-int", fields(elevated = %arguments.is_elevated(), mode = ?arguments.output_mode))]
pub(crate) async fn interactive_command_with_env<S: AsRef<str>>(
    arguments: &CommandArguments<'_, S>,
    envs: std::collections::HashMap<String, String>,
) -> Result<InteractiveChildChip, HiveLibError> {
    print_authenticate_warning(arguments)?;

    let needles = create_needles();
    let pty_system = NativePtySystem::default();
    let pty_pair = portable_pty::PtySystem::openpty(&pty_system, PtySize::default()).unwrap();
    setup_master(&pty_pair)?;

    let command_string = &format!(
        "{starting}{command} {flags} {IO_SUBS} && {ending}",
        command = arguments.command_string.as_ref(),
        flags = match arguments.output_mode {
            ChildOutputMode::Nix => "--log-format internal-json",
            ChildOutputMode::Generic | ChildOutputMode::Interactive => "",
        },
        starting = create_starting_segment(arguments, &needles.start),
        ending = create_ending_segment(arguments, &needles)
    );

    debug!("{command_string}");

    let mut command = build_command(arguments, command_string)?;

    // give command all env vars
    for (key, value) in envs {
        command.env(key, value);
    }

    let clobber_guard = aquire_stdin_lock().await;
    let _guard = StdinTermiosAttrGuard::new().map_err(HiveLibError::CommandError)?;
    let child = pty_pair
        .slave
        .spawn_command(command)
        .map_err(|x| HiveLibError::CommandError(CommandError::PortablePty(x)))?;

    // Release any handles owned by the slave: we don't need it now
    // that we've spawned the child.
    drop(pty_pair.slave);

    let reader = pty_pair
        .master
        .try_clone_reader()
        .map_err(|x| HiveLibError::CommandError(CommandError::PortablePty(x)))?;
    let master_writer = pty_pair
        .master
        .take_writer()
        .map_err(|x| HiveLibError::CommandError(CommandError::PortablePty(x)))?;

    let stderr_collection = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(10)));
    let stdout_collection = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(10)));
    let (began_tx, began_rx) = oneshot::channel::<()>();
    let (status_sender, status_receiver) = watch::channel(Status::Running);

    let stdout_handle = {
        let arguments = WatchStdoutArguments {
            began_tx,
            reader,
            needles,
            output_mode: arguments.output_mode,
            stderr_collection: stderr_collection.clone(),
            stdout_collection: stdout_collection.clone(),
            span: Span::current(),
            log_stdout: arguments.log_stdout,
            status_sender,
        };

        tokio::task::spawn_blocking(move || handle_pty_stdout(arguments))
    };

    let (write_stdin_pipe_r, write_stdin_pipe_w) =
        pipe().map_err(|x| HiveLibError::CommandError(CommandError::PosixPipe(x)))?;
    let (cancel_stdin_pipe_r, cancel_stdin_pipe_w) =
        pipe().map_err(|x| HiveLibError::CommandError(CommandError::PosixPipe(x)))?;

    tokio::task::spawn_blocking(move || {
        watch_stdin_from_user(
            &cancel_stdin_pipe_r,
            master_writer,
            &write_stdin_pipe_r,
            Span::current(),
        )
    });

    debug!("Setup threads");

    let () = began_rx
        .await
        .map_err(|x| HiveLibError::CommandError(CommandError::OneshotRecvError(x)))?;

    drop(clobber_guard);

    if arguments.keep_stdin_open {
        trace!("Sending THREAD_BEGAN_SIGNAL");

        posix_write(&cancel_stdin_pipe_w, THREAD_BEGAN_SIGNAL)
            .map_err(|x| HiveLibError::CommandError(CommandError::PosixPipe(x)))?;
    } else {
        trace!("Sending THREAD_QUIT_SIGNAL");

        posix_write(&cancel_stdin_pipe_w, THREAD_QUIT_SIGNAL)
            .map_err(|x| HiveLibError::CommandError(CommandError::PosixPipe(x)))?;
    }

    Ok(InteractiveChildChip {
        child,
        cancel_stdin_pipe_w,
        write_stdin_pipe_w,
        stderr_collection,
        stdout_collection,
        original_command: arguments.command_string.as_ref().to_string(),
        status_receiver,
        stdout_handle,
    })
}

fn print_authenticate_warning<S: AsRef<str>>(
    arguments: &CommandArguments<S>,
) -> Result<(), HiveLibError> {
    if !arguments.is_elevated() {
        return Ok(());
    }

    let _ = STATUS.lock().write_above_status(
        &format!(
            "{} | Authenticate for \"sudo {}\":\n",
            arguments
                .target
                .map_or(Ok("localhost (!)".to_string()), |target| Ok(format!(
                    "{}@{}:{}",
                    target.user,
                    target.get_preferred_host()?,
                    target.port
                )))?,
            arguments.command_string.as_ref()
        )
        .into_bytes(),
        &mut stderr(),
    );

    Ok(())
}

struct Needles {
    succeed: Arc<Vec<u8>>,
    fail: Arc<Vec<u8>>,
    start: Arc<Vec<u8>>,
}

fn create_needles() -> Needles {
    let tmp_prefix = rand::distr::SampleString::sample_string(&Alphabetic, &mut rand::rng(), 5);

    Needles {
        succeed: Arc::new(format!("{tmp_prefix}_W_Q").as_bytes().to_vec()),
        fail: Arc::new(format!("{tmp_prefix}_W_F").as_bytes().to_vec()),
        start: Arc::new(format!("{tmp_prefix}_W_S").as_bytes().to_vec()),
    }
}

fn setup_master(pty_pair: &PtyPair) -> Result<(), HiveLibError> {
    if let Some(fd) = pty_pair.master.as_raw_fd() {
        // convert raw fd to a BorrowedFd
        // safe as `fd` is dropped well before `pty_pair.master`
        let fd = unsafe { std::os::unix::io::BorrowedFd::borrow_raw(fd) };
        let mut termios =
            tcgetattr(fd).map_err(|x| HiveLibError::CommandError(CommandError::TermAttrs(x)))?;

        termios.local_flags &= !LocalFlags::ECHO;
        // Key agent does not work well without canonical mode
        termios.local_flags &= !LocalFlags::ICANON;
        // Actually quit
        termios.local_flags &= !LocalFlags::ISIG;

        tcsetattr(fd, SetArg::TCSANOW, &termios)
            .map_err(|x| HiveLibError::CommandError(CommandError::TermAttrs(x)))?;
    }

    Ok(())
}

fn build_command<S: AsRef<str>>(
    arguments: &CommandArguments<'_, S>,
    command_string: &String,
) -> Result<CommandBuilder, HiveLibError> {
    let mut command = if let Some(target) = arguments.target {
        let mut command = create_int_ssh_command(target, arguments.modifiers)?;

        // force ssh to use our pseudo terminal
        command.arg("-tt");

        command
    } else {
        let mut command = portable_pty::CommandBuilder::new("sh");

        command.arg("-c");

        command
    };

    if arguments.is_elevated() {
        command.arg(format!("sudo -u root -- sh -c '{command_string}'"));
    } else {
        command.arg(command_string);
    }

    Ok(command)
}

impl WireCommandChip for InteractiveChildChip {
    type ExitStatus = (portable_pty::ExitStatus, String);

    #[instrument(skip_all)]
    async fn wait_till_success(mut self) -> Result<Self::ExitStatus, CommandError> {
        drop(self.write_stdin_pipe_w);

        let exit_status = tokio::task::spawn_blocking(move || self.child.wait())
            .await
            .map_err(CommandError::JoinError)?
            .map_err(CommandError::WaitForStatus)?;

        debug!("exit_status: {exit_status:?}");

        self.stdout_handle
            .await
            .map_err(|_| CommandError::ThreadPanic)??;

        let status = self
            .status_receiver
            .wait_for(|value| matches!(value, Status::Done { .. }))
            .await
            .unwrap();

        let _ = posix_write(&self.cancel_stdin_pipe_w, THREAD_QUIT_SIGNAL);

        if let Status::Done { success: true } = *status {
            let logs = self
                .stdout_collection
                .lock()
                .unwrap()
                .iter()
                .rev()
                .map(|x| x.trim())
                .join("\n");

            return Ok((exit_status, logs));
        }

        debug!("child did not succeed");

        let logs = self
            .stderr_collection
            .lock()
            .unwrap()
            .iter()
            .rev()
            .join("\n");

        Err(CommandError::CommandFailed {
            command_ran: self.original_command,
            logs,
            code: format!("code {}", exit_status.exit_code()),
            reason: match *status {
                Status::Done { .. } => "marked-unsuccessful",
                Status::Running => "child-crashed-before-succeeding",
            },
        })
    }

    async fn write_stdin(&mut self, data: Vec<u8>) -> Result<(), HiveLibError> {
        trace!("Writing {} bytes to stdin", data.len());

        posix_write(&self.write_stdin_pipe_w, &data)
            .map_err(|x| HiveLibError::CommandError(CommandError::PosixPipe(x)))?;

        Ok(())
    }
}

impl StdinTermiosAttrGuard {
    fn new() -> Result<Self, CommandError> {
        let stdin = std::io::stdin();
        let stdin_fd = stdin.as_fd();

        let mut termios = tcgetattr(stdin_fd).map_err(CommandError::TermAttrs)?;
        let original_termios = termios.clone();

        termios.local_flags &= !(LocalFlags::ECHO | LocalFlags::ICANON);
        tcsetattr(stdin_fd, SetArg::TCSANOW, &termios).map_err(CommandError::TermAttrs)?;

        Ok(StdinTermiosAttrGuard(original_termios))
    }
}

impl Drop for StdinTermiosAttrGuard {
    fn drop(&mut self) {
        let stdin = std::io::stdin();
        let stdin_fd = stdin.as_fd();

        let _ = tcsetattr(stdin_fd, SetArg::TCSANOW, &self.0);
    }
}

fn create_int_ssh_command(
    target: &Target,
    modifiers: SubCommandModifiers,
) -> Result<portable_pty::CommandBuilder, HiveLibError> {
    let mut command = portable_pty::CommandBuilder::new("ssh");
    command.args(target.create_ssh_args(modifiers, false, false)?);
    command.arg(target.get_preferred_host()?.to_string());
    Ok(command)
}

#[cfg(test)]
mod tests {
    use aho_corasick::AhoCorasick;
    use tokio::sync::oneshot::error::TryRecvError;

    use crate::commands::pty::output::handle_rawmode_data;

    use super::*;
    use std::assert_matches::assert_matches;

    #[test]
    fn test_rawmode_data() {
        let aho_corasick = AhoCorasick::builder()
            .ascii_case_insensitive(false)
            .match_kind(aho_corasick::MatchKind::LeftmostFirst)
            .build(["START_NEEDLE", "SUCCEEDED_NEEDLE", "FAILED_NEEDLE"])
            .unwrap();
        let mut stderr = vec![];
        let (began_tx, mut began_rx) = oneshot::channel::<()>();
        let mut began_tx = Some(began_tx);
        let (status_sender, _) = watch::channel(Status::Running);

        // each "Bla" is 4 bytes.
        let buffer = "bla bla bla START_NEEDLE bla bla bla".as_bytes();
        let mut raw_mode_buffer = vec![];

        // handle 1 "bla"
        assert_matches!(
            handle_rawmode_data(
                &mut stderr,
                buffer,
                4,
                &mut raw_mode_buffer,
                &aho_corasick,
                &status_sender,
                &mut began_tx
            ),
            Ok(SearchFindings::None)
        );
        assert_matches!(began_rx.try_recv(), Err(TryRecvError::Empty));
        assert!(began_tx.is_some());
        assert_eq!(raw_mode_buffer, b"bla ");
        assert_matches!(*status_sender.borrow(), Status::Running);

        let buffer = &buffer[4..];

        // handle 2 "bla"'s and half a "START_NEEDLE"
        let n = 4 + 4 + 6;
        assert_matches!(
            handle_rawmode_data(
                &mut stderr,
                buffer,
                n,
                &mut raw_mode_buffer,
                &aho_corasick,
                &status_sender,
                &mut began_tx
            ),
            Ok(SearchFindings::None)
        );
        assert_matches!(began_rx.try_recv(), Err(TryRecvError::Empty));
        assert!(began_tx.is_some());
        assert_matches!(*status_sender.borrow(), Status::Running);
        assert_eq!(raw_mode_buffer, b"bla bla bla START_");

        let buffer = &buffer[n..];

        // handle rest of the data
        let n = buffer.len();
        assert_matches!(
            handle_rawmode_data(
                &mut stderr,
                buffer,
                n,
                &mut raw_mode_buffer,
                &aho_corasick,
                &status_sender,
                &mut began_tx
            ),
            Ok(SearchFindings::Started)
        );
        assert_matches!(began_rx.try_recv(), Ok(()));
        assert_matches!(began_tx, None);
        assert_eq!(raw_mode_buffer, b"bla bla bla START_NEEDLE bla bla bla");
        assert_matches!(*status_sender.borrow(), Status::Running);

        // test failed needle
        let buffer = "bla FAILED_NEEDLE bla".as_bytes();
        let mut raw_mode_buffer = vec![];

        let n = buffer.len();
        assert_matches!(
            handle_rawmode_data(
                &mut stderr,
                buffer,
                n,
                &mut raw_mode_buffer,
                &aho_corasick,
                &status_sender,
                &mut began_tx
            ),
            Ok(SearchFindings::Terminate)
        );
        assert_matches!(*status_sender.borrow(), Status::Done { success: false });

        // test succeed needle
        let buffer = "bla SUCCEEDED_NEEDLE bla".as_bytes();
        let mut raw_mode_buffer = vec![];
        let (status_sender, _) = watch::channel(Status::Running);

        let n = buffer.len();
        assert_matches!(
            handle_rawmode_data(
                &mut stderr,
                buffer,
                n,
                &mut raw_mode_buffer,
                &aho_corasick,
                &status_sender,
                &mut began_tx
            ),
            Ok(SearchFindings::Terminate)
        );
        assert_matches!(*status_sender.borrow(), Status::Done { success: true });
    }
}
