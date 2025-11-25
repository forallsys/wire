{ flake }:
let
  nixpkgs = import flake.inputs.nixpkgs {
    system = "x86_64-linux";
  };

  vmNode =
    index:
    nixpkgs.lib.nameValuePair "bench-vm-${builtins.toString index}" {
      deployment = {
        targetPort = 2000 + index;
        targetHost = "localhost";
      };

      imports = [
        ./vm.nix
      ];

      _module.args = {
        index = builtins.toString index;
      };

      deployment.keys = builtins.listToAttrs (
        builtins.map (
          index:
          nixpkgs.lib.nameValuePair "key-${builtins.toString index}" {
            keyFile = ./key.txt;
            # 80% of keys pre activation, 20% post activation.
            uploadAt = if index <= (200 * 0.8) then "pre-activation" else "post-activation";
          }
        ) (nixpkgs.lib.range 0 200)
      );

      nixpkgs.hostPlatform = "x86_64-linux";
    };
in
{
  meta.nixpkgs = nixpkgs;
}
// builtins.listToAttrs (builtins.map vmNode (nixpkgs.lib.range 0 20))
