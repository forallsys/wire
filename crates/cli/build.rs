// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

fn main() {
    if let Some(change_id) = get_change_id() {
        println!("cargo:rustc-env=GIT_COMMIT_ID={change_id}");
    }
}

fn get_change_id() -> Option<String> {
    let change = std::env::var("WIRE_CHANGE_ID").unwrap_or_else(|_| "dirty".to_string());

    if change.trim().is_empty() {
        return None;
    }

    Some(change.trim().to_string())
}
