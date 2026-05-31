# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

{
  lib,
  inputs,
  ...
}:
let
  inherit (lib)
    mapAttrsToList
    flatten
    ;
in
{
  config.perSystem =
    {
      pkgs,
      self',
      system,
      ...
    }:
    let
      benchDirFileset = lib.fileset.toSource {
        root = ../..;
        fileset = lib.fileset.union ./. (
          lib.fileset.fileFilter (
            file: (file.hasExt "nix") || (file.hasExt "txt") || (file.hasExt "lock")
          ) ../.
        );
      };

      nodes =
        builtins.listToAttrs (
          builtins.map (index: {
            value = {
              imports = [
                ./vm.nix
              ];

              _module.args = {
                index = builtins.toString index;
              };
            };
            name = "node_${builtins.toString index}";
          }) (lib.range 0 (import ./num-nodes.nix))
        )
        // {
          deployer = {
            imports = [
              ./vm.nix
            ];

            environment.systemPackages = [
              pkgs.git

              (pkgs.writeShellScriptBin "setup-benchmark" ''
                mkdir -p $HOME/wire
                cp -r ${benchDirFileset}/*-source/* $HOME/wire

                cp -r $HOME/wire/bench/wire-flake $HOME/wire-flake
                cp -r $HOME/wire/bench/colmena-flake $HOME/colmena-flake

                chmod -R +w $HOME/*

                cd $HOME/wire
                git init .
                git add -A
              '')

              (pkgs.writeShellScriptBin "run-benchmark" ''
                bench_dir=$HOME/wire/bench

                wire_args="apply test --path $bench_dir/wire -vv --ssh-accept-host -p 10"
                wire_args_flake="apply test --path $HOME/wire-flake -vv --ssh-accept-host -p 10"
                wire_args_flake_exp_client="apply test --path $HOME/wire-flake -vv --ssh-accept-host -p 10 --experimental-nix-client"

                colmena_args="apply test --config $bench_dir/colmena/hive.nix -v -p 10"
                colmena_args_flake="apply test --config $HOME/colmena-flake/flake.nix -v -p 10"

                ${lib.getExe pkgs.hyperfine} --warmup 2 --runs 5 \
                  --export-markdown stats.md \
                  --export-json run.json \
                  "${lib.getExe self'.packages.wire-small} $wire_args_flake" -n "wire@HEAD - flake" \
                  "${lib.getExe self'.packages.wire-small} $wire_args_flake_exp_client" -n "wire@HEAD & --experimental-nix-client - flake" \
                  "${lib.getExe' inputs.colmena_benchmarking.packages.x86_64-linux.colmena "colmena"} $colmena_args_flake" \
                      -n "colmena@pinned - flake" \
                  "${lib.getExe self'.packages.wire-small} $wire_args" -n "wire@HEAD - hive.nix"
              '')
            ];

            _module.args = {
              index = "deployer";
            };
          };
        };

      evalConfig = import (pkgs.path + "/nixos/lib/eval-config.nix");

      evalVM =
        module:
        evalConfig {
          inherit system;
          modules = [ module ];
        };
    in
    {
      checks.bench = pkgs.testers.runNixOSTest {
        inherit nodes;

        name = "benchmark";

        defaults =
          _:
          let
            fetchLayer =
              input:
              let
                subLayers = if input ? inputs then map fetchLayer (builtins.attrValues input.inputs) else [ ];
              in
              [
                input.outPath
              ]
              ++ subLayers;
          in
          {
            virtualisation.additionalPaths = flatten [
              (mapAttrsToList (_: val: (evalVM val).config.system.build.toplevel.drvPath) nodes)
              (mapAttrsToList (_: fetchLayer) inputs)
            ];

            nix.settings.experimental-features = [
              "nix-command"
              "flakes"
            ];
          };
        node.specialArgs = {
          snakeOil = import "${pkgs.path}/nixos/tests/ssh-keys.nix" pkgs;
          inherit (self'.packages) wire-small-dev;
        };
        skipTypeCheck = true;
        testScript = ''
          start_all()

          for i in range(0,${builtins.toString (import ./num-nodes.nix)}):
              machine = globals().get(f"node_{i}")
              machine.wait_for_unit("sshd.service") # type: ignore

          node_deployer.succeed("setup-benchmark");
          node_deployer.succeed("run-benchmark");

          node_deployer.copy_from_vm("run.json")
          node_deployer.copy_from_vm("stats.json")
        '';
      };
    };
}
