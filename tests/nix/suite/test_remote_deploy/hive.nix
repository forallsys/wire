# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

let
  inherit (import ../utils.nix { testName = "test_keys-@IDENT@"; }) makeHive mkHiveNode;
in
makeHive {
  meta = {
    nixpkgs = import <nixpkgs> { localSystem = "x86_64-linux"; };

    specialArgs = {
      message = "second";
    };

    nodeSpecialArgs = {
      receiver-third.message = "third";
    };
  };

  receiver = mkHiveNode { hostname = "receiver"; } {
    environment.etc."identity".text = "first";

    # test node pinging
    deployment.target.hosts = [
      "unreachable-1"
      "unreachable-2"
      "unreachable-3"
      "unreachable-4"
      "receiver"
    ];
  };

  receiver-second = mkHiveNode { hostname = "receiver"; } (
    { message, ... }:
    {
      environment.etc."identity".text = message;
      deployment.target.host = "receiver";
    }
  );

  receiver-third = mkHiveNode { hostname = "receiver"; } (
    { message, ... }:
    {
      environment.etc."identity".text = message;
      deployment.target.host = "receiver";
    }
  );

  receiver-unreachable = mkHiveNode { hostname = "receiver"; } {
    # test node pinging
    deployment.target.hosts = [
      "completely-unreachable"
    ];
  };
}
