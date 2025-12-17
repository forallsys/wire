---
comment: true
title: Basic Hive & Deployment
description: Creating a basic hive and deploying changes to the virtual machine.
---

# Basic Hive & Deployment

{{ $frontmatter.description }}

## Editing `hive.nix`

Open a text editor and edit `hive.nix`. You should copy this example, which imports
the npins sources we added. It also calls `makeHive`, and gives wire `nixpkgs`
from npins as well.

```nix:line-numbers [hive.nix]
let
  # import npins sources
  sources = import ./npins;
  # import `wire` from npins sources
  wire = import sources.wire;
in
wire.makeHive {
  # give wire nixpkgs from npins
  meta.nixpkgs = import sources.nixpkgs { };

  # we'll edit this part
}
```

Lets check out what wire sees with `wire show`.

```sh
[nix-shell]$ wire show
 INFO eval_hive: evaluating hive HiveNix("/home/marsh/scratch/wire-tutorial/hive.nix")
 WARN use --json to output something scripting suitable
Summary: 0 total node(s), totalling 0 keys (0 distinct).
Note: Listed connections are tried from Left to Right

```

The line `nodes: {}` means there is no "nodes" in our hive.

## Adding The First Node

Lets add the virtual machine as a node to the hive with the name
`virtual-machine`. Additionally, we will add `deployment.target`, recalling we
forwarded sshd `virtual-machine:22` to the port `localhost:2222`:

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in
wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  virtual-machine = { pkgs, ... }: { # [!code ++]
    deployment.target = { # [!code ++]
        port = 2222; # [!code ++]
        hosts = [ "localhost" ]; # [!code ++]
    }; # [!code ++]

    nixpkgs.hostPlatform = "x86_64-linux"; # [!code ++]
  }; # [!code ++]
}
```

## A naive `wire apply`

If we tried to run `wire apply` on our hive at this stage, it likely won't work.
If you've used NixOS before, you'll notice that many important options are
missing. But let's try anyway:

```sh
[nix-shell]$ wire apply
ERROR apply{goal=Switch on=}:goal{node=virtual-machine}: lib::hive::node: Failed to execute `Evaluate the node`
Error:   × 1 node(s) failed to apply.

Error:
  × node virtual-machine failed to apply
  ├─▶ wire::Evaluate
  │
  │     × failed to evaluate `--file /home/marsh/scratch/wire-tutorial/hive.nix topLevels.virtual-machine` from the context
  │     │ of a hive.
  │
  ╰─▶ nix --extra-experimental-features nix-command --extra-experimental-features flakes eval --json  --file /home/marsh/scratch/
      wire-tutorial/hive.nix topLevels.virtual-machine --log-format internal-json failed (reason: known-status) with code 1 (last 20
      lines):
      error:
             … while evaluating '(evaluateNode node).config.system.build.toplevel' to select 'drvPath' on it
               at /nix/store/5pfz0v479gnciac17rcqi2gwyz8pl4s0-source/runtime/evaluate.nix:65:23:
                 64|
                 65|   getTopLevel = node: (evaluateNode node).config.system.build.toplevel.drvPath;
                   |                       ^
                 66| in

             … while calling the 'head' builtin
               at /nix/store/n3d1ricw0cb5jd8vvfym6ig0mw7x7sv9-source/lib/attrsets.nix:1701:13:
               1700|           if length values == 1 || pred here (elemAt values 1) (head values) then
               1701|             head values
                   |             ^
               1702|           else

             (stack trace truncated; use '--show-trace' to show the full trace)

             error:
             Failed assertions:
             - The ‘fileSystems’ option does not specify your root file system.
             - You must set the option ‘boot.loader.grub.devices’ or 'boot.loader.grub.mirroredBoots' to make the system bootable.
      trace: evaluation warning: system.stateVersion is not set, defaulting to 25.11. Read why this matters on https://nixos.org/
      manual/nixos/stable/options.html#opt-system.stateVersion.

```

The command complained about not defining any fileSystems or a boot loader.
The `${sources.nixpkgs}/nixos/modules/virtualisation/qemu-vm.nix` imported in
`vm.nix` does
extra work to make our virtual machine work, which we are currently missing.

## Importing `vm.nix`

Lets import our `vm.nix` to this hive to fix our evaluation errors.
Additionally, add a new package such as `vim` to our configuration:

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in
wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  virtual-machine = { pkgs, ... }: {
    deployment.target = {
        port = 2222;
        hosts = [ "localhost" ];
    };

    imports = [ # [!code ++]
        ./vm.nix # [!code ++]
    ]; # [!code ++]

    environment.systemPackages = [ pkgs.vim ]; # [!code ++]

    nixpkgs.hostPlatform = "x86_64-linux";
  };
}
```

## Our first deploy

Trying our basic `wire apply` again with these changes:

```sh
[nix-shell]$ wire apply
...
 INFO lib::nix_log: stopping the following units: boot.mount
 INFO lib::nix_log: NOT restarting the following changed units: systemd-fsck@dev-disk-by\x2dlabel-ESP.service
 INFO lib::nix_log: activating the configuration...
 INFO lib::nix_log: setting up /etc...
 INFO lib::nix_log: restarting systemd...
 INFO lib::nix_log: reloading user units for root...
 INFO lib::nix_log: restarting sysinit-reactivation.target
 INFO lib::nix_log: reloading the following units: dbus.service
 INFO lib::nix_log: the following new units were started: boot.automount, sysinit-reactivation.target, systemd-tmpfiles-resetup.service
 INFO apply{goal=Switch on=}:goal{node=virtual-machines}: lib::hive::node: Executing step `Upload key @ PostActivation`
 INFO apply{goal=Switch on=}: wire::apply: Successfully applied goal to 1 node(s): [Name("virtual-machines")]
```

Now, lets confirm these changes were applied to the virtual machine by executing
`vim` in the virtual machine window:

```sh [Virtual Machine]
[root@wire-tutorial:~]# vim --version
VIM - Vi IMproved 9.1 (2024 Jan 02, compiled Jan 01 1980 00:00:00)
```

Nice! You successfully deployed a new NixOS configuration to a **remote host**!

::: info
This followed common steps of adding the node's `deployment.target` details and
importing it's pre-existing NixOS configuration (in this case, `vm.nix`), a
pattern you'll be using a lot if you chose to adopt wire.
:::

In the next section, we'll cover how to deploy secrets / keys to our remote node.
