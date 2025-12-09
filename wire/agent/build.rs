// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

extern crate prost_build;

fn main() {
    prost_build::compile_protos(&["src/keys.proto"], &["src/"]).unwrap();
}
