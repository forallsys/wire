---
comment: true
title: Install wire
description: How to install wire tool.
---

# Install wire

{{ $frontmatter.description }}

::: info

The `wire` binary and the `wire.makeHive` function are tightly coupled, so it is
recommended that you use the same version for both.

:::

It is recommended you stick to either using a tagged version of wire, or the `stable` branch which tracks the latest stable tag.

## Binary Cache

You should enable the [garnix binary cache](https://garnix.io/docs/caching) _before_
continuing otherwise you will be compiling from source:

::: code-group
<<< @/snippets/tutorial/cache.conf [nix.conf]
<<< @/snippets/tutorial/cache.nix [configuration.nix]
:::

## Installation through flakes

When using flakes, you should install wire through the same input you create
your hive from, sourced from the `stable` branch.

::: code-group
<<< @/snippets/guides/installation/flake.nix [flake.nix]
:::

## Installation through npins

With npins you may allow it to use release tags instead of the `stable`
branch.

Using npins specifically is not required, you can pin your sources in any way
you'd like, really.

```sh
$ npins add github forallsys wire --branch stable
```

Alternatively, you can use a tag instead:

```sh
$ npins add github forallsys wire --at v1.1.1
```

Then, use this pinned version of wire for both your `hive.nix` and `shell.nix`:

<<< @/snippets/guides/installation/shell.nix{8} [shell.nix]
<<< @/snippets/guides/installation/hive.nix [hive.nix]
