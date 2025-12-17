---
comment: true
title: Deployment Keys Basics
description: Deploy a age-encrypted secret with wire tool.
---

# Deployment Keys Basics

{{ $frontmatter.description }}

::: tip
For this tutorial we will be using [`age`](https://github.com/FiloSottile/age),
but other encryption CLI tools work just as well such as GnuPG.
:::

## Installing age

Alter your shell.nix to include age:

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
    pkgs.age # [!code ++]
  ];

  shellHook = ''
    export NIX_PATH="nixpkgs=${sources.nixpkgs.outPath}"
  '';
}
```

Quit and re-open your shell, and confirm age is now available:

```sh
[nix-shell]$ exit
exit
$ nix-shell
[nix-shell]$ age --version
1.2.1

```

## Encrypting a secret

First create an age private key:

```sh
[nix-shell]$ age-keygen -o key.txt
Public key: age1j08s3kmr8zw4w8k99vs4nut5mg03dm8nfuaajuekdyzlujxply5qwsv4g0

```

::: details
Further details on how age works can be found on in the
[age manual](https://man.archlinux.org/man/age.1.en.txt).
:::

Now, lets encrypt the words `"!! encrypted string !!"` with age and save it to the
file `top-secret.age`.

We will use a pipeline to echo the encrypted string into
age, and use `age-keygent -y` to give age the public key we generated, then we
use the redirection operator to save the encrypted data to `top-secret.age`.

```sh
[nix-shell]$ echo "encrypted string!" | age --encrypt --recipient $(age-keygen -y key.txt) > top-secret.age
```

## Adding an age-encrypted key

Now, lets combine our previous command-sourced key with `age`. Pass the
arguments `age --decrypt --identity key.txt ./top-secret.age` to wire:

```nix:line-numbers [secrets.nix]
{
  deployment.keys = {
    # ...

    "top-secret" = { # [!code ++]
      source = [ # [!code ++]
        "age" # [!code ++]
        "--decrypt" # [!code ++]
        "--identity" # [!code ++]
        "key.txt" # [!code ++]
        "${./top-secret.age}" # [!code ++]
      ]; # [!code ++]
    }; # [!code ++]
  };
}
```

One `wire apply keys` later, and you have successfully deployed an encrypted
key:

```sh [Virtual Machine]
[root@wire-tutorial:~]# cat /run/keys/top-secret
encrypted string!

```
