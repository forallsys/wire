---
comment: true
title: Creating a Virtual Machine
description: Creating a NixOS virtual machine to use as a deployment target.
---

# Creating a Virtual Machine

{{ $frontmatter.description }}

## Creating a `vm.nix`

For this step, you'll need your ssh public key, which you can obtain from
`ssh-add -L`.

Open a text editor and edit `vm.nix`. Place in it this basic NixOS
virtual machine configuration, which enables openssh and forwards it's 22 port:

```nix:line-numbers [vm.nix]
let
  sources = import ./npins;
in
{
  imports = [ "${sources.nixpkgs}/nixos/modules/virtualisation/qemu-vm.nix" ];

  networking.hostName = "wire-tutorial";

  users.users.root = {
    initialPassword = "root";
    openssh.authorizedKeys.keys = [
      # I made this a nix syntax error so you're forced to deal with it!
      <your ssh public-key as a string>
    ];
  };

  boot = {
    loader = {
      systemd-boot.enable = true;
      efi.canTouchEfiVariables = true;
    };

    kernelParams = [ "console=ttyS0" ];

    boot.growPartition = true;
  };

  # enable openssh
  services = {
    openssh = {
      enable = true;
      settings.PermitRootLogin = "yes";
    };

    getty.autologinUser = "root";
  };

  virtualisation = {
    graphics = false;
    useBootLoader = true;

    # use a 5gb disk
    diskSize = 5 * 1024;

    # grow the filesystem to fit the 5 gb we reserved
    fileSystems."/".autoResize = true;

    # forward `openssh` port 22 to localhost:2222.
    forwardPorts = [
      {
        from = "host";
        host.port = 2222;
        guest.port = 22;
      }
    ];
  };

  system.stateVersion = "23.11";
}
```

If you like, you may take a moment to understand each line of this
configuration.

## Building & Running the virtual machine

Open a separate Terminal tab/window/instance, ensuring you enter the development
shell with `nix-shell`.
Then, build the virtual machine with a bootloader,
taking our `vm.nix` as the nixos configuration.

```sh
$ nix-shell
[nix-shell]$ nix-build '<nixpkgs/nixos>' -A vmWithBootLoader -I nixos-config=./vm.nix
```

::: tip HELP

If you got an error such as

```
error: The option `...' in `...' is already declared in `...'.
```

make sure you ran the above command in the `nix-shell`!

:::

Building the virtual machine can take some time, but once it completes, start it
by running:

```sh
[nix-shell]$ ./result/bin/run-wire-tutorial-vm
```

You will see boot-up logs fly across the screen and eventually you will be placed
into shell inside the virtual machine.

```sh [Virtual Machine]
running activation script...
setting up /etc...

Welcome to NixOS 25.11 (Xantusia)!

[  OK  ] Created slice Slice /system/getty.
[  OK  ] Created slice Slice /system/modprobe.
...
<<< Welcome to NixOS 25.11pre861972.88cef159e47c (x86_64) - hvc0 >>>

Run 'nixos-help' for the NixOS manual.

wire-tutorial login: root (automatic login)

[root@wire-tutorial:~]#

```

::: details
Further details on how the above commands work can be found at
[nix.dev](https://nix.dev/tutorials/nixos/nixos-configuration-on-vm.html#creating-a-qemu-based-virtual-machine-from-a-nixos-configuration)
:::

## Summary

Congratulations, you created a virtual machine in your terminal.
We'll be deploying to this virtual machine, so keep the
terminal instance open.

::: info
From now on, commands ran inside the virtual machine will be lead with the
following prompt:

```sh [Virtual Machine]
[root@wire-tutorial:~]#

```

:::

::: tip
If you ever want to quit the virtual machine, run the command `poweroff`.
:::
