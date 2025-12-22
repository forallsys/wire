{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.wire.url = "github:forallsys/wire";

  outputs = {
    self,
    nixpkgs,
    wire,
    ...
  } @ inputs: {
    wire = wire.makeHive {
      # Give wire our nixosConfigurations
      inherit (self) nixosConfigurations;

      meta = {
        nixpkgs = import nixpkgs {localSystem = "x86_64-linux";};
      };

      node-a.deployment = {
        tags = [
          # some tags
        ];

        # ...
      };
    };

    nixosConfigurations = {
      node-a = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        specialArgs = {inherit inputs;};
        modules = [
          wire.nixosModules.default
          {
            nixpkgs.hostPlatform = "x86_64-linux";

            # you can put deployment options here too!
            deployment.target = "some-hostname";
          }
        ];
      };

      some-other-host = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        specialArgs = {inherit inputs;};
        modules = [
          {
            nixpkgs.hostPlatform = "x86_64-linux";
          }
        ];
      };
    };
  };
}
