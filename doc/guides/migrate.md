---
comment: true
title: Migrate to wire
description: How-to migrate from other tools to wire tool.
---

# Migrate to wire

{{ $frontmatter.description }}

Migrate from...

- [Colmena](#from-colmena)
- [`nixos-rebuild`](#from-nixos-rebuild)

## From Colmena

If you're familiar with colmena, wire will hopefully come quickly to you! (or,
atleast that was the intention when writing it!). There are a few changes you
should know:

- [You don't have to use a root user](/guides/non-root-user.html)
- `apply-local` does not exist, `apply` will apply locally when appropriate
- [Many options have been aliased to nicer names](/reference/module.html)
  (ie, `deployment.targetUser` <=> `deployment.target.user`)
- You may pass a list of hosts to `deployment.targetHost` (no more fiddling with
  your hive whenever DNS is down, for example)
- `--path` optionally takes a flakeref! You can pass `--path github:foo/bar`,
  `--path git+file:///...`, `--path https://.../main.tar.gz`, etc.
  (plain paths like `--path ~/my-hive` still work as always)

::: tip
You should also follow [installation](/guides/installation) to install the
binary.
:::

### Convert a Hive as a Flake

```nix [flake.nix]
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    colmena.url = "github:zhaofengli/colmena"; # [!code --]
    wire.url = "github:mrshmllow/wire/stable"; # [!code ++]
  };
  outputs =
    { nixpkgs, colmena, ... }:
    {
      colmenaHive = colmena.lib.makeHive { # [!code --]
      wire = wire.lib.makeHive { # [!code ++]
        # ..
      };
    };
}
```

### Convert a Hive with npins

::: tip
You should also follow [installation](/guides/installation) to setup
npins and install the binary.
:::

Unlike colmena, you must call `makeHive` directly even in non-flake hives.

```nix [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in
{ # [!code --]
wire.makeHive { # [!code ++]

  meta.nixpkgs = <nixpkgs>; # [!code --]
  meta.nixpkgs = import sources.nixpkgs { }; # [!code ++]

  # ...
}
```

Replacing `<nixpkgs>` with a pinned source is optional, but you should
probably use one if you ask me \:)

## From `nixos-rebuild`

You can keep using `nixos-rebuild` alongside wire!

Follow the instructions in [the relevant page](/guides/flakes/nixos-rebuild.html).
