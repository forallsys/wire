{
  inputs.wire.url = "git+file:///root/wire";

  outputs =
    { wire, ... }:
    {
      wire = wire.makeHive (import "${wire}/bench/default.nix" { flake = wire; });
    };
}
