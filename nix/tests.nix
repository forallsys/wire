{
  perSystem =
    {
      craneLib,
      pkgs,
      commonArgs,
      ...
    }:
    let
      tests = craneLib.buildPackage (
        {
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          doCheck = false;

          doNotPostBuildInstallCargoBinaries = true;

          buildPhase = ''
            cargo test --no-run
          '';

          installPhaseCommand = ''
            mkdir -p $out
            cp $(ls target/debug/deps/{wire,lib,agent}-* | grep -v "\.d") $out
          '';
        }
        // commonArgs
      );
    in
    {
      packages.cargo-tests = pkgs.writeShellScriptBin "run-tests" ''
        set -e
        for item in "${tests}"/*; do
            echo "running $item"
            "$item"
        done
      '';
    };
}
