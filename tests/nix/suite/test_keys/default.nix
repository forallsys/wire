# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

{
  wire.testing.test_keys = {
    nodes.deployer = {
      _wire.deployer = true;
      _wire.receiver = true;
    };
    nodes.receiver = {
      _wire.receiver = true;
    };
    testScript = builtins.readFile ./script.py;
  };
}
