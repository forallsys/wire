# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2024-2025 wire Contributors

{
  self,
  config,
  lib,
  inputs,
  ...
}:
let
  inherit (lib)
    mkOption
    mapAttrsToList
    flatten
    cartesianProduct
    ;
  inherit (lib.types)
    submodule
    lines
    attrsOf
    anything
    lazyAttrsOf
    ;
  cfg = config.wire.testing;
in
{
  imports = [
    ./suite/test_remote_deploy
    ./suite/test_local_deploy
    ./suite/test_keys
    ./suite/test_stdin
    ./suite/test_rollback
  ];
  options.wire.testing = mkOption {
    type = attrsOf (
      submodule (
        { name, ... }:
        {
          options = {
            nodes = mkOption {
              type = lazyAttrsOf anything;
            };
            testScript = mkOption {
              type = lines;
              default = '''';
              description = "test script for runNixOSTest";
            };
            testDir = mkOption {
              default = "${self}/tests/nix/suite/${name}";
              readOnly = true;
            };
          };
        }
      )
    );
    description = "A set of test cases for wire VM testing suite";
  };

  config.perSystem =
    {
      pkgs,
      self',
      inputs',
      ...
    }:
    let
      nixNixpkgsCombos = cartesianProduct {
        nixpkgs = [
          inputs'.nixpkgs
          inputs'.nixpkgs_current_stable
          # inputs'.nixpkgs_prev_stable
        ];
        # TODO: Update once #126 is solved.
        nix = [
          # "nix"
          "lix"
        ];
        testName = builtins.attrNames cfg;
      };
      mkTest =
        {
          testName,
          opts,
          nixpkgs,
        }:
        let
          # TODO: Update once #126 is solved.
          nixPackage = nixpkgs.legacyPackages.lix;
          sanitiseName =
            str: lib.strings.sanitizeDerivationName (builtins.replaceStrings [ "." ] [ "_" ] str);
          identifier = sanitiseName "${nixpkgs.legacyPackages.lib.trivial.release}-${nixPackage.name}";
          path = "tests/nix/suite/${testName}";

          flakeDirFileset = lib.fileset.toSource {
            root = ../..;
            fileset = lib.fileset.union ./. (
              lib.fileset.fileFilter (file: (file.hasExt "nix") || (file.hasExt "lock")) ../..
            );
          };

          injectedFlakeDir = pkgs.runCommand "injected-flake-dir" { } ''
            cp -r ${flakeDirFileset} $out
            chmod -R +w $out
            substituteInPlace $out/${path}/hive.nix --replace-fail @IDENT@ ${identifier}
          '';
        in
        rec {
          name = "vm-${testName}-${identifier}";
          value = pkgs.testers.runNixOSTest {
            inherit (opts) nodes;
            inherit name;
            defaults =
              {
                pkgs,
                ...
              }:
              let
                hive = builtins.scopedImport {
                  __nixPath = _b: null;
                  __findFile = _path: name: if name == "nixpkgs" then pkgs.path else throw "oops!!";
                } "${injectedFlakeDir}/${path}/hive.nix";
                nodes = mapAttrsToList (_: val: val.config.system.build.toplevel.drvPath) hive.nodes;
                # fetch **all** dependencies of a flake
                # it's called fetchLayer because my naming skills are awful
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
                imports = [ ./test-opts.nix ];
                nix = {
                  nixPath = [ "nixpkgs=${pkgs.path}" ];
                  settings.substituters = lib.mkForce [ ];
                  package = nixPackage;
                };

                environment.systemPackages = [ pkgs.ripgrep ];
                environment.variables.XDG_RUNTIME_DIR = "/tmp";
                virtualisation.memorySize = 4096;
                virtualisation.additionalPaths = flatten [
                  injectedFlakeDir
                  nodes
                  (mapAttrsToList (_: fetchLayer) inputs)
                ];
              };
            node.specialArgs = {
              testName = name;
              snakeOil = import "${pkgs.path}/nixos/tests/ssh-keys.nix" pkgs;
              inherit (opts) testDir;
              inherit (self'.packages) wire-small-dev;
            };
            # NOTE: there is surely a better way of doing this in a more
            # "controlled" manner, but until a need is asked for, this will remain
            # as is.
            testScript = ''
              start_all()

              TEST_DIR="${injectedFlakeDir}/${path}"

              ${builtins.readFile ./tools.py}
            ''
            + lib.concatStringsSep "\n" (mapAttrsToList (_: value: value._wire.testScript) value.nodes)
            + opts.testScript;
          };
        };
    in
    {
      checks = builtins.listToAttrs (
        builtins.map (
          {
            nixpkgs,
            testName,
            ...
          }:
          let
            opts = cfg.${testName};
          in
          mkTest {
            inherit
              testName
              opts
              nixpkgs
              ;
          }
        ) nixNixpkgsCombos
      );
    };
}
