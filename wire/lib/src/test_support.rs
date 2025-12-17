// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{
    env,
    fs::{self, create_dir},
    io,
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{LazyLock, atomic::AtomicU16},
    thread,
    time::{Duration, Instant},
};

use tempdir::TempDir;

use crate::hive::node::Target;

pub fn make_flake_sandbox(path: &Path) -> Result<TempDir, io::Error> {
    let tmp_dir = TempDir::new("wire-test")?;

    Command::new("git")
        .args(["init", "-b", "tmp"])
        .current_dir(tmp_dir.path())
        .status()?;

    for entry in fs::read_dir(path)? {
        let entry = entry?;

        fs::copy(entry.path(), tmp_dir.as_ref().join(entry.file_name()))?;
    }

    let root = path.parent().unwrap().parent().unwrap().parent().unwrap();

    create_dir(tmp_dir.as_ref().join("module/"))?;

    fs::copy(
        root.join(Path::new("runtime/evaluate.nix")),
        tmp_dir.as_ref().join("evaluate.nix"),
    )?;
    fs::copy(
        root.join(Path::new("runtime/module/config.nix")),
        tmp_dir.as_ref().join("module/config.nix"),
    )?;
    fs::copy(
        root.join(Path::new("runtime/module/options.nix")),
        tmp_dir.as_ref().join("module/options.nix"),
    )?;
    fs::copy(
        root.join(Path::new("runtime/module/default.nix")),
        tmp_dir.as_ref().join("module/default.nix"),
    )?;
    fs::copy(
        root.join(Path::new("runtime/makeHive.nix")),
        tmp_dir.as_ref().join("makeHive.nix"),
    )?;
    fs::copy(
        root.join(Path::new("flake.lock")),
        tmp_dir.as_ref().join("flake.lock"),
    )?;

    Command::new("git")
        .args(["add", "-A"])
        .current_dir(tmp_dir.path())
        .status()?;

    Command::new("nix")
        .args(["flake", "lock"])
        .current_dir(tmp_dir.path())
        .status()?;

    Ok(tmp_dir)
}

pub(crate) struct CargoTestVirtualMachine {
    pub(crate) target: Target,
    child: Child,
}

// corresponds to `tests/tests.nix`, that file needs to be updated
// to support new tests being added
static TEST_COUNTER: LazyLock<AtomicU16> = LazyLock::new(|| AtomicU16::new(0));
const VM_START_WAIT: std::time::Duration = Duration::from_secs(10);
const VM_PORT_BASE: u16 = 2000;

fn wait_for_port(port: u16) {
    let start = Instant::now();

    while start.elapsed() < VM_START_WAIT {
        match TcpStream::connect(("localhost", port)) {
            Ok(_) => return,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    panic!("Test vm failed to open port {port}");
}

// corresponds to `tests/tests.nix`, do not change values without updating that :)
pub fn test_with_vm() -> CargoTestVirtualMachine {
    let mut vms_path = PathBuf::from(env::var("WIRE_TEST_VM").unwrap());
    let index = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    vms_path.push(format!("{index}/bin/run-cargo-vm-{index}-vm"));

    let child = Command::new(vms_path.clone())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_for_port(VM_PORT_BASE + index);

    let target = Target::new("localhost".into(), "root".into(), VM_PORT_BASE + index);

    CargoTestVirtualMachine { target, child }
}

impl Drop for CargoTestVirtualMachine {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
