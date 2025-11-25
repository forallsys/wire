{
  inputs.wire.url = "git+file:///home/marsh/project/wire";

  outputs =
    { wire, ... }:
    let
    in
    {
      wire = wire.makeHive (import "${wire}/bench/default.nix" { flake = wire; });
    };
}
