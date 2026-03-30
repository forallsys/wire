{
  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";
    flake-compat.url = "https://git.lix.systems/lix-project/flake-compat/archive/main.tar.gz";
    git-hooks.url = "github:cachix/git-hooks.nix";
    systems.url = "github:nix-systems/default";
    crane.url = "github:ipetkov/crane";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix.url = "github:nix-community/fenix";
    fenix.inputs.nixpkgs.follows = "nixpkgs";
    treefmt-nix.url = "github:numtide/treefmt-nix";

    # determines systems available for deployment
    linux-systems.url = "github:nix-systems/default-linux";

    # testing inputs
    nixpkgs_current_stable.url = "github:NixOS/nixpkgs/nixos-25.11";

    # benchmarking
    colmena_benchmarking.url = "github:zhaofengli/colmena/v0.4.0";
  };
  outputs =
    {
      self,
      flake-parts,
      systems,
      git-hooks,
      crane,
      treefmt-nix,
      ...
    }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        git-hooks.flakeModule
        treefmt-nix.flakeModule
        ./nix/hooks.nix # pre-commit hooks
        ./nix/utils.nix # utility functions
        ./nix/shells.nix
        ./nix/tests.nix
        ./crates/cli
        ./crates/key_agent
        ./doc
        ./tests/nix
        ./runtime
        ./bench/runner.nix
      ];
      systems = import systems;

      flake = {
        makeHive = import ./runtime/makeHive.nix;
        hydraJobs =
          let
            inherit (inputs.nixpkgs) lib;
          in
          {
            packages = {
              inherit (self.packages.x86_64-linux) docs;
            }
            // lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (system: {
              inherit (self.packages.${system}) wire wire-small;
            });

            tests = lib.filterAttrs (n: _: (lib.hasPrefix "vm" n)) self.checks.x86_64-linux;
            inherit (self) devShells;
          };
      };

      perSystem =
        {
          pkgs,
          inputs',
          config,
          lib,
          ...
        }:
        {
          _module.args = {
            toolchain = inputs'.fenix.packages.complete;
            craneLib = (crane.mkLib pkgs).overrideToolchain config._module.args.toolchain.toolchain;
            inherit self;
          };
          treefmt = {
            programs = {
              # rfc style
              nixfmt.enable = true;
              # docs only
              alejandra.enable = true;
              rustfmt.enable = true;
              prettier.enable = true;
              protolint.enable = true;
              taplo.enable = true;
              ruff-format.enable = true;
            };
            settings.formatter = {
              nixfmt.excludes = [ "doc/snippets/*.nix" ];
              prettier.excludes = [ ".sqlx/*" ];
              alejandra = {
                includes = lib.mkForce [ "doc/snippets/*.nix" ];
              };
            };
          };
        };
    };
}
