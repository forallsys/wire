{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.wire.url = "github:forallsys/wire";

  outputs = inputs @ {
    nixpkgs,
    wire,
    ...
  }: {
    wire = wire.makeHive {
      meta = {
        nixpkgs = import nixpkgs {
          localSystem = "x86_64-linux";
        };
        specialArgs = {
          inherit inputs;
        };
      };

      defaults = {
        # ...
      };

      node-a = {
        nixpkgs.hostPlatform = "x86_64-linux";

        # ...
      };
    };
  };
}
