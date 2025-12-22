---
comment: true
title: Preparing Repo & Shell
description: Adding npins sources and a nix development shell.
---

# Preparing Repo & Shell

{{ $frontmatter.description }}

## Initialising with Git & `npins`

First, lets create an adhoc shell to bring these two tools into our $PATH.

```sh
$ nix-shell -p git npins
[nix-shell]$ git --version
git version 2.51.0
[nix-shell]$ npins --version
npins 0.3.1
```

Great! Now lets use Git & `npins` to create a new Git repo and initialise it.
`npins init` may take a while to download `nixpkgs`.

```sh
[nix-shell]$ git init wire-tutorial
Initialized empty Git repository in /home/.../wire-tutorial/.git/
[nix-shell]$ cd wire-tutorial/
[nix-shell]$ npins init
[INFO ] Welcome to npins!
[INFO ] Creating `npins` directory
[INFO ] Writing default.nix
[INFO ] Writing initial lock file (empty)
[INFO ] Successfully written initial files to 'npins/sources.json'.
```

This has created a pinned version of `nixpkgs` for us to use in our wire hive.

## Adding wire as a dependency

We can now need to tell `npins` to use `forallsys/wire` as a dependency.

```sh
[nix-shell]$ npins add github forallsys wire --branch stable
[INFO ] Adding 'wire' …
    repository: https://github.com/forallsys/wire.git
    pre_releases: false
    submodules: false
    version: v0.4.0
    revision: f33d80c15b17c85d557d533441609a59a2210941
    hash: 0wgah341hvjpvppkgwjrj50rvzf56ccmjz720xsl3mw38h9nn6sr
    frozen: false
```

Great, now lets confirm the two dependencies we have added to this `npins`
project:

```sh
[nix-shell]$ npins show
nixpkgs: (git repository)
    repository: https://github.com/pkpbynum/nixpkgs.git
    branch: pb/disk-size-bootloader
    submodules: false
    revision: da2060bdc1c9bc35acc4eafa265ba6b6c64f9926
    url: https://github.com/pkpbynum/nixpkgs/archive/da2060bdc1c9bc35acc4eafa265ba6b6c64f9926.tar.gz
    hash: 0j07gvnm7c5mzw1313asa8limzbmsbnsd02dcw22ing8fg3vbb7g
    frozen: false

wire: (git release tag)
    repository: https://github.com/forallsys/wire.git
    pre_releases: false
    submodules: false
    version: v0.4.0
    revision: f33d80c15b17c85d557d533441609a59a2210941
    hash: 0wgah341hvjpvppkgwjrj50rvzf56ccmjz720xsl3mw38h9nn6sr
    frozen: false
```

## Creating a `shell.nix`

Open a text editor to edit `shell.nix` in the `wire-tutorial` directory.

```nix:line-numbers [shell.nix]
let
  sources = import ./npins;
  pkgs = import sources.nixpkgs { };
  wire = import sources.wire;
in
pkgs.mkShell {
  packages = [
    wire.packages.x86_64-linux.wire-small
    pkgs.npins
    pkgs.git
  ];

  shellHook = ''
    export NIX_PATH="nixpkgs=${sources.nixpkgs.outPath}"
  '';
}
```

You should now `exit` to quit the old shell, and
enter a new shell with `nix-shell`. Since we added wire as a package, our new
shell should have wire in the $PATH:

```sh
[nix-shell]$ exit
exit
$ cd wire-tutorial/
$ nix-shell
[nix-shell]$ wire --version
wire 0.5.0
Debug: Hive::SCHEMA_VERSION 0

```
