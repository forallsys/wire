// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

use std::{
    fs::{self, create_dir},
    io,
    path::Path,
    process::Command,
};

use tempdir::TempDir;

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
