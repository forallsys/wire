---
comment: true
title: Install wire
description: How to install wire tool.
---

# {{ $frontmatter.title }}

{{ $frontmatter.description }}

::: info

The `wire` binary and the `wire.makeHive` function are tightly coupled, so it is
recommended that you use the same version for both.

:::

It is recommended you stick to either using a tagged version of wire, or the `stable` branch which tracks the latest stable tag.

## Binary Cache

You should enable the [garnix binary cache](https://garnix.io/docs/caching).

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
$ npins add github mrshmllow wire --branch stable
```

Alternatively, you can use a tag instead:

```sh
$ npins add github mrshmllow wire --at v1.0.0-beta.0
```

Then, use this pinned version of wire for both your `hive.nix` and `shell.nix`:

::: code-group
<<< @/snippets/guides/installation/shell.nix{8} [shell.nix]
<<< @/snippets/guides/installation/hive.nix [hive.nix]
:::
