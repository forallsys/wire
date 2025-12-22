{system, ...}: let
  wire = import (
    # [!code ++]
    builtins.fetchTarball "https://github.com/forallsys/wire/archive/refs/heads/trunk.tar.gz" # [!code ++]
  ); # [!code ++]
in {
  environment.systemPackages = [
    wire.packages.${system}.wire # [!code ++]
  ];

  # ...
}
