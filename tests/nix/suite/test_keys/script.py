# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from test_driver.machine import Machine
    from tools import collect_store_objects, assert_store_not_poisoned

    deployer: Machine = None  # type: ignore[invalid-assignment]
    receiver: Machine = None  # type: ignore[invalid-assignment]
    TEST_DIR = ""

# typing-end

deployer_so = collect_store_objects(deployer)
receiver_so = collect_store_objects(receiver)

# build receiver with no keys
deployer.succeed(
    f"wire apply --no-progress --on receiver --path {TEST_DIR}/hive.nix --no-keys --ssh-accept-host -vvv >&2"
)

receiver.wait_for_unit("sshd.service")

# --no-keys should never push a key
receiver.fail("test -f /run/keys/source_string_name")
deployer.fail("test -f /run/keys/source_string_name")

# key services are created
receiver.succeed("systemctl cat source_string_name-key.service")

_, is_failed = receiver.execute("systemctl is-failed source_string_name-key.service")
assert (
    is_failed == "inactive\n"
), f"source_string_name-key.service must be inactive before key exists ({is_failed})"


def test_keys(target, target_object, non_interactive):
    if non_interactive:
        deployer.succeed(
            f"wire apply keys --on {target} --no-progress --path {TEST_DIR}/hive.nix --non-interactive --ssh-accept-host -vvv >&2"
        )
    else:
        deployer.succeed(
            f"wire apply keys --on {target} --no-progress --path {TEST_DIR}/hive.nix --ssh-accept-host -vvv >&2"
        )

    keys = [
        (
            "/run/keys/source_string_name",
            "hello_world_source",
            "root root 600",
            "source_string_name",
        ),
        ("/etc/keys/file", "hello_world_file", "root root 644", "file"),
        (
            "/home/owner/some/deep/path/command",
            "hello_world_command",
            "owner owner 644",
            "command",
        ),
        (
            "/run/keys/environment",
            "string_from_environment",
            "root root 600",
            "environment",
        ),
    ]

    for path, value, permissions, name in keys:
        # test existence & value
        source_string = target_object.succeed(f"cat {path}")
        assert value in source_string, f"{path} has correct contents ({target})"

        stat = target_object.succeed(f"stat -c '%U %G %a' {path}").rstrip()
        assert permissions == stat, f"{path} has correct permissions ({target})"


def perform_routine(target, target_object, non_interactive):
    test_keys(target, target_object, non_interactive)

    # only check systemd units on receiver since deployer apply's are one time only
    if target == "receiver":
        target_object.succeed("systemctl start source_string_name-key.path")
        target_object.succeed("systemctl start command-key.path")
        target_object.wait_for_unit("source_string_name-key.service")
        target_object.wait_for_unit("command-key.service")

    # Mess with the keys to make sure that every push refreshes the permissions
    target_object.succeed("echo 'incorrect_value' > /run/keys/source_string")
    target_object.succeed("chown 600 /etc/keys/file")
    # Test having a key that doesn't exist mixed with keys that do
    target_object.succeed("rm /home/owner/some/deep/path/command")

    if target == "receiver":
        _, is_failed = target_object.execute("systemctl is-active command-key.service")
        assert (
            is_failed == "failed\n"
        ), f"command-key.service is failed after deletion ({is_failed})"

    # Test keys twice to ensure the operation is idempotent,
    # especially around directory creation.
    test_keys(target, target_object, non_interactive)


perform_routine("receiver", receiver, True)
perform_routine("deployer", deployer, True)
perform_routine("receiver", receiver, False)
perform_routine("deployer", deployer, False)

new_deployer_store_objects = collect_store_objects(deployer).difference(deployer_so)
new_receiver_store_objects = collect_store_objects(receiver).difference(receiver_so)

# no one should have any keys introduced by the operation
for node, objects in [
    (deployer, new_deployer_store_objects),
    (receiver, new_receiver_store_objects),
]:
    assert_store_not_poisoned(node, "hello_world_source", objects)
    assert_store_not_poisoned(node, "hello_world_file", objects)
    assert_store_not_poisoned(node, "hello_world_command", objects)
    assert_store_not_poisoned(node, "string_from_environment", objects)
