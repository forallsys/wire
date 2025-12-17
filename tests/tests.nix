{ inputs, ... }:
{
  perSystem =
    {
      craneLib,
      pkgs,
      lib,
      commonArgs,
      system,
      cargo-testing-vms,
      cargo-testing-exports,
      self',
      ...
    }:
    let
      evalConfig = import (pkgs.path + "/nixos/lib/eval-config.nix");
      tests = craneLib.buildPackage (
        {
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          doCheck = false;

          doNotPostBuildInstallCargoBinaries = true;

          buildPhase = ''
            cargo test --no-run
          '';

          installPhaseCommand = ''
            mkdir -p $out
            cp $(ls target/debug/deps/{wire,lib,key_agent}-* | grep -v "\.d") $out
          '';
        }
        // commonArgs
      );

      snakeOil = import "${pkgs.path}/nixos/tests/ssh-keys.nix" pkgs;
    in
    {
      packages.cargo-tests = pkgs.writeShellScriptBin "run-tests" ''
        set -e

        ${cargo-testing-exports}

        for item in "${tests}"/*; do
            echo "running $item"
            "$item"
        done
      '';

      _module.args = {
        cargo-testing-exports = ''
          export WIRE_TEST_VM="${cargo-testing-vms}"
          export WIRE_PUSHABLE_PATH="${self'.packages.agent}"
          export WIRE_SSH_KEY="${snakeOil.snakeOilEd25519PrivateKey}"
        '';

        cargo-testing-vms =
          let
            mkVM =
              index:
              evalConfig {
                inherit system;
                modules = lib.singleton {
                  imports = [ "${inputs.nixpkgs}/nixos/modules/virtualisation/qemu-vm.nix" ];

                  networking.hostName = "cargo-vm-${builtins.toString index}";

                  boot = {
                    loader = {
                      systemd-boot.enable = true;
                      efi.canTouchEfiVariables = true;
                      timeout = 0;
                    };

                    kernelParams = [ "console=ttyS0" ];
                  };

                  services = {
                    openssh = {
                      enable = true;
                      settings = {
                        PermitRootLogin = "without-password";
                      };
                    };

                    getty.autologinUser = "root";
                  };

                  virtualisation = {
                    graphics = false;

                    diskSize = 5024;
                    diskImage = null;

                    # testing for pushing is hard without this
                    # useBootLoader = true;
                    useNixStoreImage = true;
                    writableStore = true;

                    forwardPorts = [
                      {
                        from = "host";
                        host.port = 2000 + index;
                        guest.port = 22;
                      }
                    ];
                  };

                  users.users.root.openssh.authorizedKeys.keys = [ snakeOil.snakeOilEd25519PublicKey ];

                  users.users.root.initialPassword = "root";

                  system.stateVersion = "23.11";
                };
              };
          in
          pkgs.linkFarm "vm-forest" (
            builtins.map (index: {
              path = (mkVM index).config.system.build.vm;
              name = builtins.toString index;
              # Updated with every new test that uses a VM
            }) (lib.range 0 1)
          );
      };
    };
}
