{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    wire.url = "github:mrshmllow/wire/stable";

    # alternatively, you can use a tag instead:
    # wire.url = "github:mrshmllow/wire/v1.1.0";

    systems.url = "github:nix-systems/default";
  };

  outputs = {
    nixpkgs,
    wire,
    systems,
    ...
  }: let
    forAllSystems = nixpkgs.lib.genAttrs (import systems);
  in {
    wire = wire.makeHive {
      meta.nixpkgs = import nixpkgs {localSystem = "x86_64-linux";};

      # Continue to next How-To guide to fill this section
    };

    devShells = forAllSystems (
      system: let
        pkgs = nixpkgs.legacyPackages.${system};
      in {
        default = pkgs.mkShell {
          buildInputs = [
            wire.packages.${system}.wire
          ];
        };
      }
    );
  };
}
