---
comment: true
title: Deployment Keys Basics
description: Deploy some basic secrets with wire tool.
---

# Deployment Keys Basics

{{ $frontmatter.description }}

## Creating a `secrets.nix`

Lets create a NixOS module that will contain our secret keys, and import it:

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in
wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  virtual-machine = {
    deployment.target = {
        port = 2222;
        hosts = [ "localhost" ];
    };

    imports = [
        ./vm.nix
        ./secrets.nix # [!code ++]
    ];

    environment.systemPackages = [ pkgs.vim ];

    nixpkgs.hostPlatform = "x86_64-linux";
  };
}
```

```nix:line-numbers [secrets.nix]
{
  deployment.keys = {
    # the key's unique name is `"basic.txt"`.
    "basic.txt" = {
      # In this key's case, the source is a literal string:
      source = ''
        Hello World
      '';
    };
  };
}
```

::: details
Further details on the `deployment.keys` options can be found
[in the reference](/reference/module.html#deployment-keys)
:::

Once we deploy this new configuration to the virtul machine,
`/run/keys/basic.txt` will be created with the contents of the key.

```sh
[nix-shell]$ wire apply keys
 WARN lib::nix_log: Store URL: ssh://root@localhost
(root@localhost) Password:

```

```sh [Virtual Machine]
[root@wire-tutorial:~]# cat /run/keys/basic.txt
Hello World

```

You successfully deployed your first, albeit not-so-secret, secret key! Let's
move on from literal-text keys and use something a bit more powerful.

## File-sourced keys <Badge type="info">Optional</Badge>

This section is optional to try, but you can also pass `deployment.keys.<name>.source`
a file path. It's contents is read and treated as literal text.

```sh
$ echo hello world > very-important-secret.txt
```

```nix:line-numbers [secrets.nix]
{
  deployment.keys = {
    # ...

    "very-important-secret.txt" = { # [!code ++]
      source = ./very-important-secret.txt; # [!code ++]
    }; # [!code ++]
  };
}
```

```sh [Virtual Machine]
[root@wire-tutorial:~]# cat /run/keys/very-important-secret.txt
hello world

```

## Command-sourced keys

Command-sourced keys are where the real power of wire keys lie. By passing a
list of strings, wire will execute them as a command and create a key out of it's `stdout`.

Because the command's output is never written to the nix store, these can be
considered real secrets.

To create a basic example, update your `secrets.nix` to include a secret that
echos "hello world":

```nix:line-numbers [secrets.nix]
{
  deployment.keys = {
    # ...

    "command.txt" = { # [!code ++]
      source = [ # [!code ++]
        "echo" # [!code ++]
        "hello world" # [!code ++]
      ]; # [!code ++]
    }; # [!code ++]
  };
}
```

After a quick `wire deploy secrets`, the `/run/keys/command.txt` file is
created:

```sh [Virtual Machine]
[root@wire-tutorial:~]# cat /run/keys/command.txt
hello world

```

Hopefully you can see the potential of command-sourced keys, as these are the
basic building block of how we achieve encrypted secrets with wire.
