{
  inputs = {
    # ...
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    wire.url = "github:forallsys/wire"; # [!code ++]
  };

  outputs = inputs @ {
    # ...
    nixpkgs,
    wire, # [!code ++]
    ...
  }: {
    nixosConfigurations.my-system = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      specialArgs = {inherit inputs;};
      modules = [
        # ...
        (
          {system, ...}: {
            environment.systemPackages = [
              wire.packages.${system}.wire # [!code ++]
            ];
          }
        )
      ];
    };
  };
}
