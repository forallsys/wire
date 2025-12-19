{
  perSystem =
    {
      buildRustProgram,
      system,
      ...
    }:
    {
      packages = {
        agent = buildRustProgram {
          name = "wire-key-agent";
          pname = "wire-tool-key-agent-${system}";
          cargoExtraArgs = "-p key-agent";
        };
      };
    };
}
