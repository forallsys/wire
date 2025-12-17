---
comment: true
title: Nix Setup
description: Installing npins, nix, and enabling the binary cache.
---

# Nix Setup

{{ $frontmatter.description }}

::: warning
This page is for the purposes for the **Tutorial**.
You should read [How-to Guides - Install wire](/guides/installation.html) for installing wire for
regular use.
:::

## Nix Installation

You should install nix if you do not have it on your system already.
There are detailed steps to installing Nix on [nix.dev](https://nix.dev/install-nix).

By the end of the installation, you should see something like this:

```sh
$ nix --version
nix (Nix) 2.11.0
```

## Binary Cache

Because wire can be heavy to compile, it is distributed with a [binary
cache](https://wiki.nixos.org/wiki/Binary_Cache).

You must enable the [garnix binary cache](https://garnix.io/docs/caching) or you
will be compiling everything from source:

::: code-group
<<< @/snippets/tutorial/cache.conf [nix.conf]
<<< @/snippets/tutorial/cache.nix [configuration.nix]
:::
