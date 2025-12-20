# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from test_driver.machine import Machine


def collect_store_objects(machine: Machine) -> set[str]:
    return set(machine.succeed("ls /nix/store").strip().split("\n"))


def assert_store_not_poisoned(machine: Machine, poison: str, objects: set[str]):
    paths = list(map(lambda n: f"/nix/store/{n}", objects))

    machine.succeed("which rg")
    machine.fail(f"rg '{poison}' {' '.join(paths)}")
