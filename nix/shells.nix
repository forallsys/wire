{
  perSystem =
    {
      config,
      lib,
      craneLib,
      pkgs,
      ...
    }:
    let
      cfg = config.pre-commit;
    in
    {
      # Adapted from
      # https://github.com/cachix/git-hooks.nix/blob/dcf5072734cb576d2b0c59b2ac44f5050b5eac82/flake-module.nix#L66-L78
      devShells.default = craneLib.devShell {
        packages = lib.flatten [
          cfg.settings.enabledPackages
          cfg.settings.package

          pkgs.pnpm
          pkgs.nodejs
          pkgs.sqlx-cli
          pkgs.sqlite
          pkgs.turso
          pkgs.zstd
          pkgs.uv
        ];

        PROTOC = lib.getExe pkgs.protobuf;
        shellHook = builtins.concatStringsSep "\n" [
          cfg.installationScript
          ''
            export WIRE_TEST_DIR=$(realpath ./tests/rust)
          ''
        ];
      };
    };
}
