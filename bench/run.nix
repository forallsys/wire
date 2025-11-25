{ inputs, ... }:
{
  perSystem =
    {
      pkgs,
      lib,
      self',
      system,
      ...
    }:
    {
      packages =
        let
          evalConfig = import (pkgs.path + "/nixos/lib/eval-config.nix");
          mkVM =
            index:
            evalConfig {
              inherit system;
              modules = [
                ./vm.nix
                {
                  _module.args = {
                    index = toString index;
                  };
                }
              ];
            };

          forest = pkgs.linkFarm "vm-forest" (
            builtins.map (index: {
              path = (mkVM index).config.system.build.vm;
              name = builtins.toString index;
            }) (lib.range 0 20)
          );
        in
        {
          bench-runner = pkgs.writeShellScriptBin "bench-runner" ''
            set -e

            ${lib.toShellVars {
              inherit forest;
            }}

            bench_dir=$(mktemp -d)
            cp -r ${../.}/* $bench_dir
            chmod 777 -R $bench_dir/
            bench_dir=$bench_dir/bench

            export NIX_PATH="nixpkgs=${inputs.nixpkgs}"

            echo "setting up vms..."

            for i in {0..20}
            do
              "$forest/$i/bin/run-bench-vm" &
            done

            echo "sleeping"
            sleep 10
            echo "awake"

            wire_args="apply test --path $bench_dir/wire -vv --ssh-accept-host -p 10"
            wire_args_no_prog="apply test --path $bench_dir/wire -vv --ssh-accept-host --no-progress -p 10"
            wire_args_flake="apply test --path $bench_dir/wire-flake -vv --ssh-accept-host -p 10"

            colmena_args="apply test --config $bench_dir/colmena/hive.nix -v -p 10"
            colmena_args_flake="apply test --config $bench_dir/colmena-flake/flake.nix -v -p 10"

            ${lib.getExe pkgs.hyperfine} --warmup 1 --show-output --runs 5 \
              --export-markdown stats.md \
              --export-json run.json \
              "${lib.getExe self'.packages.wire-small} $wire_args_flake" -n "wire@HEAD - flake" \
              "${
                lib.getExe (builtins.getFlake "github:mrshmllow/wire/stable").packages.${system}.wire-small
              } $wire_args_flake" -n "wire@stable - flake" \
              "${lib.getExe self'.packages.wire-small} $wire_args_no_prog" -n "wire@HEAD - no_prog" \
              "${lib.getExe self'.packages.wire-small} $wire_args" -n "wire@HEAD" \
              "${
                lib.getExe (builtins.getFlake "github:mrshmllow/wire/stable").packages.${system}.wire-small
              } $wire_args" -n "wire@stable" \
              "${lib.getExe' inputs.colmena_benchmarking.packages.x86_64-linux.colmena "colmena"} $colmena_args" \
                  -n "colmena@pinned"
          '';
        };
    };
}
