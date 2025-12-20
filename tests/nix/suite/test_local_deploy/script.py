# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from test_driver.machine import Machine

    deployer: Machine = None  # type: ignore[invalid-assignment]
    TEST_DIR = ""

deployer.succeed(
    f"wire apply --on deployer --no-progress --path {TEST_DIR}/hive.nix --no-keys -vvv >&2"
)
deployer.succeed("test -f /etc/a")
