{
  inputs.wire.url = "git+file:///root/wire";

  outputs =
    { wire, ... }:
    {
      colmenaHive = wire.inputs.colmena_benchmarking.lib.makeHive (
        import "${wire}/bench/default.nix" { flake = wire; }
      );
    };
}
