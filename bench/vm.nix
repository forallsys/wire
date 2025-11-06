{ lib, index, ... }:
let
  flake = import ../default.nix;
in
{
  _module.args = {
    index = lib.mkDefault (builtins.getEnv "INDEX");
  };

  imports = [ "${flake.inputs.nixpkgs}/nixos/modules/virtualisation/qemu-vm.nix" ];

  networking.hostName = "bench";

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
    # useBootLoader = true;

    diskSize = 5024;
    diskImage = null;

    forwardPorts = [
      {
        from = "host";
        host.port = 2000 + lib.toIntBase10 index;
        guest.port = 22;
      }
    ];
  };

  users.users.root.openssh.authorizedKeys.keys = [
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPSvOZoSGVEpR6eTDK9OJ31MWQPF2s8oLc8J7MBh6nez marsh@maple"
  ];

  users.users.root.initialPassword = "root";

  system.stateVersion = "23.11";
}
