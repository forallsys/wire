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
          name = "agent";
          pname = "wire-tool-agent-${system}";
          cargoExtraArgs = "-p agent";
        };
      };
    };
}
