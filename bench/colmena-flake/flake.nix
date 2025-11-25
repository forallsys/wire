{
  inputs.wire.url = "git+file:///home/marsh/project/wire";

  outputs =
    { wire, ... }:
    let
    in
    {
      colmenaHive = wire.inputs.colmena_benchmarking.lib.makeHive (
        import "${wire}/bench/default.nix" { flake = wire; }
      );
    };
}
