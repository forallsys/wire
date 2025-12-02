# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

let
  inherit (import ../utils.nix { testName = "test_rollback-@IDENT@"; }) makeHive mkHiveNode;
in
makeHive {
  meta.nixpkgs = import <nixpkgs> { localSystem = "x86_64-linux"; };

  receiver = mkHiveNode { hostname = "receiver"; } (
    { lib, ... }:
    {
      environment.etc."identity".text = "first";
    }
  );

  receiver-broken = mkHiveNode { hostname = "receiver"; } (
    { lib, ... }:
    {
      environment.etc."identity".text = "second";
      deployment.target.hosts = [ "receiver" ];

      services.openssh.enable = lib.mkForce false;
    }
  );
}
