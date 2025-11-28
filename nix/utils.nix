{
  perSystem =
    {
      pkgs,
      lib,
      craneLib,
      commonArgs,
      ...
    }:
    {
      _module.args = {
        commonArgs =
          let
            inherit (lib.fileset)
              toSource
              unions
              ;
            src = toSource {
              root = ../.;
              fileset = unions [
                ../.cargo
                ../.sqlx
                ../wire
                ../Cargo.toml
                ../Cargo.lock
              ];
            };

            commonArgs = {
              inherit src;
              strictDeps = true;
              WIRE_TEST_DIR = ../tests/rust;
              PROTOC = lib.getExe pkgs.protobuf;
            };
          in
          commonArgs;
        buildRustProgram =
          extra:
          let
            args = builtins.removeAttrs extra [ "name" ];
          in
          craneLib.buildPackage (
            {
              cargoArtifacts = craneLib.buildDepsOnly commonArgs;
            }
            // args
            // commonArgs
          );
      };
    };
}
