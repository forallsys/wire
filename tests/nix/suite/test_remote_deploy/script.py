# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

from typing import TYPE_CHECKING
from typing import Callable, ContextManager

if TYPE_CHECKING:
    from test_driver.machine import Machine

    deployer: Machine = None  # type: ignore[invalid-assignment]
    receiver: Machine = None  # type: ignore[invalid-assignment]

    TEST_DIR = ""

    # https://github.com/NixOS/nixpkgs/blob/d10d9933b1c206f9b2950e5e1d68268c5ed0a3c7/nixos/lib/test-script-prepend.py#L43
    subtest: Callable[[str], ContextManager[None]] = None  # type: ignore[invalid-assignment]

# typing-end

with subtest("Test unreachable hosts"):
    deployer.fail(
        f"wire apply --on receiver-unreachable --no-progress --path {TEST_DIR}/hive.nix --no-keys -vvv >&2"
    )

with subtest("Check basic apply: Interactive"):
    deployer.succeed(
        f"wire apply --on receiver --no-progress --path {TEST_DIR}/hive.nix --no-keys --ssh-accept-host -vvv >&2"
    )

    identity = receiver.succeed("cat /etc/identity")
    assert identity == "first", "Identity of first apply wasn't as expected"

with subtest("Check basic apply: NonInteractive"):
    deployer.succeed(
        f"wire apply --on receiver-third --no-progress --path {TEST_DIR}/hive.nix --no-keys --ssh-accept-host --non-interactive -vvv >&2"
    )

    identity = receiver.succeed("cat /etc/identity")
    assert identity == "third", "Identity of non-interactive apply wasn't as expected"

with subtest("Check boot apply"):
    first_system = receiver.succeed("readlink -f /run/current-system")

deployer.succeed(
    f"wire apply boot --on receiver-second --no-progress --path {TEST_DIR}/hive.nix --no-keys --ssh-accept-host -vvv >&2"
)

_first_system = receiver.succeed("readlink -f /run/current-system")
assert first_system == _first_system, (
    "apply boot without --reboot changed /run/current-system"
)

# with subtest("Check /etc/identity after reboot"):
#   receiver.reboot()
#
#   identity = receiver.succeed("cat /etc/identity")
#   assert identity == "second", "Identity didn't change after second apply"

# with subtest("Check --reboot"):
#   deployer.succeed(f"wire apply boot --on receiver-third --no-progress --path {TEST_DIR}/hive.nix --reboot --no-keys -vvv >&2")
#
#   identity = receiver.succeed("cat /etc/identity")
#   assert identity == "third", "Identity didn't change after third apply"
