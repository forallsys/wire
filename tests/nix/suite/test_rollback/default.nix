# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

{
  wire.testing.test_remote_deploy = {
    nodes.deployer = {
      _wire.deployer = true;
    };
    nodes.receiver = {
      _wire.receiver = true;
    };
    testScript = ''
      with subtest("Deploy broken config"):
        deployer.fail(f"wire apply --on receiver --no-progress --path {TEST_DIR}/hive.nix --no-keys -vvv >&2")

      with subtest("Configuration must revert"):
        receiver.wait_for_unit("sshd.service")
    '';
  };
}
