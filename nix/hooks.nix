{
  perSystem =
    {
      toolchain,
      config,
      lib,
      ...
    }:
    {
      pre-commit = {
        settings = {
          hooks = {
            statix.enable = true;
            deadnix = {
              enable = true;
              settings.edit = true;
            };
            zizmor.enable = true;
            clippy = {
              enable = true;
              settings.extraArgs = "--tests";
              packageOverrides = {
                inherit (toolchain) cargo clippy;
              };
            };
            cargo-check = {
              enable = true;
              package = toolchain.cargo;
            };
            fmt = {
              enable = true;
              name = "nix fmt";
              entry = "${lib.getExe config.formatter} --no-cache";
            };
            typos = {
              enable = true;
              settings = {
                configPath = "typos.toml";
              };
            };

          };

        };

      };
    };

}
