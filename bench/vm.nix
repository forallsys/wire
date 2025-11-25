{
  lib,
  index,
  modulesPath,
  pkgs,
  ...
}:
let
  flake = import ../default.nix;
  snakeOil = import "${pkgs.path}/nixos/tests/ssh-keys.nix" pkgs;
in
{
  imports = [
    "${flake.inputs.nixpkgs}/nixos/modules/virtualisation/qemu-vm.nix"
    "${modulesPath}/virtualisation/qemu-vm.nix"
    "${modulesPath}/testing/test-instrumentation.nix"
  ];

  networking.hostName = "node_${index}";

  boot = {
    loader = {
      systemd-boot.enable = true;
      efi.canTouchEfiVariables = true;
    };
  };

  environment.variables.XDG_RUNTIME_DIR = "/tmp";

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
    memorySize = 4096;
  };

  # It's important to note that you should never ever use this configuration
  # for production. You are risking a MITM attack with this!
  programs.ssh.extraConfig = ''
    Host *
      StrictHostKeyChecking no
      UserKnownHostsFile /dev/null
  '';

  users.users.root.openssh.authorizedKeys.keys = [ snakeOil.snakeOilEd25519PublicKey ];
  systemd.tmpfiles.rules = [
    "C+ /root/.ssh/id_ed25519 600 - - - ${snakeOil.snakeOilEd25519PrivateKey}"
  ];

  nix = {
    nixPath = [ "nixpkgs=${pkgs.path}" ];
    settings.substituters = lib.mkForce [ ];
    package = pkgs.lix;
  };
}
