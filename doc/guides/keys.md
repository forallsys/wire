---
comment: true
title: Manage Secrets
description: Manage keys, secrets, files, and other out-of-store paths with wire Tool.
---

# Manage Secrets

{{ $frontmatter.description }}

## Introduction

wire Tool is very unopinionated as to how you encrypt your secrets, wire only
handles pushing and setting up permissions of your key files.

The `source` of your key can be a literal string (unencrypted), a path
(unencrypted), or a command that wire runs to evaluate the key. Programs that
work well with wire keys include:

- GPG
- [sops](https://github.com/getsops/sops) ([Example](#with-sops))
- [Age](https://github.com/FiloSottile/age)
- Anything that non-interactively decrypts to `stdout`.

### Prerequisites

wire uses a Rust binary to receive encrypted key data, so your deploying
user must be trusted or you must add garnix as a trusted public key:

```nix
{ config, ... }:
{
  nix.settings.trusted-users = [
    config.deployment.target.user # [!code ++]
  ];
}
```

Otherwise, you may see errors such as:

```
error: cannot add path '/nix/store/...-wire-tool-key_agent-x86_64-linux-...' because it lacks a signature by a trusted key
```

This is a requirement because `nix copy` is used to copy the binary.
As a benefit to this approach, key deployments are significantly faster!

### A Trivial "Key"

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  node-1 = {
    deployment.key."file.txt" = {
      source = ''
        Hello World!
      '';
    };
  };
}
```

```sh
[user@node-1]$ cat /run/keys/file.txt
Hello World!
```

### Encrypting with GPG

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  node-1 = {
    deployment.key."file.txt" = {
      source = [
        "gpg"
        "--decrypt"
        "${./secrets/file.txt.gpg}"
      ];
    };
  };
}
```

```sh
[user@node-1]$ cat /run/keys/file.txt
Hello World!
```

### Encrypting with KeepassXC

A simple example of extracting a KeepassXC attachment into a wire key.
You must pass the password through stdin as the command must be non-interactive.
Note that the `--stdout` is important as wire expects the command to output the key to stdout.

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  node-1 = {
    deployment.key."file.txt" = {
      source = [
        "bash"
        "-c"
        ''cat ~/pass | keepassxc-cli attachment-export --stdout ~/.local/share/keepass/database.kdbx test 'file.txt'''
      ];
    };
  };
}
```

```sh
[user@node-1]$ cat /run/keys/file.txt
Hello World!
```

### A Plain Text File

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  node-1 = {
    deployment.key."file.txt" = {
      # using this syntax will enter the file into the store, readable by
      # anyone!
      source = ./file.txt;
    };
  };
}
```

## Persistence

wire defaults `destDir` to `/run/keys`. `/run/` is held in memory and will not
persist past reboot. Change
[`deployment.key.<name>.destDir`](/reference/module#deployment-keys-name-destdir)
to something like `/etc/keys` if you need secrets every time the machine boots.

## Upload Order

By default wire will upload keys before the system is activated. You can
force wire to upload the key after the system is activated by setting
[`deployment.keys.<name>.uploadAt`](/reference/module#deployment-keys-name-uploadat)
to `post-activation`.

## Permissions and Ownership

wire secrets are owned by user & group `root` (`0600`). You can change these
with the `user` and `group` option.

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  node-1 = {
    deployment.key."file.txt" = {
      source = [
        "gpg"
        "--decrypt"
        "${./secrets/file.txt.gpg}"
      ];

      user = "my-user";
      group = "my-group";
    };
  };
}
```

## Further Examples

### With Sops

With some sops file:

```yaml:line-numbers [secret.yaml]
hive:
    some_secret: XXXXXXXXXXXXXXXXXXXXXXX
something:
    another_secret: XXXXXXXXXXXXXXXXXXXXXXX
```

You can easily create a function to grab values out of your encrypted sops file:

<<< @/snippets/guides/sops-example.nix [hive.nix]

### Using Keys With Services

You can access the full absolute path of any key with
`config.deployment.keys.<name>.path` (auto-generated and read-only).

Keys also have a `config.deployment.keys.<name>.service` property
(auto-generated and read-only), which represent systemd services that you can
`require`, telling systemd there is a hard-dependency on that key for the
service to run.

Here's an example with the Tailscale service:

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  node-1 = {config, ...}: {
    services.tailscale = {
      enable = true;
      # use deployment key path directly
      authKeyFile = config.deployment.keys."tailscale.key".path;
    };

    deployment.keys."tailscale.key" = {
      keyCommand = ["gpg" "--decrypt" "${./secrets/tailscale.key.gpg}"];
    };

    # The service will not start unless the key exists.
    systemd.services.tailscaled-autoconnect.requires = [
      config.deployment.keys."tailscale.key".service
    ];
  };
}
```

### Scoping a Key to a service account

Additionally you can scope the key to the user that the service runs under, to
further reduce duplication using the `config` argument. Here's an example of
providing a certificate that is only readable by the caddy service.

```nix:line-numbers [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  some-web-server = {config, ...}: {
    deployment.keys."some.host.pem" = {
      keyCommand = ["gpg" "--decrypt" "${./some.host.pem.gpg}"];
      destDir = "/etc/keys";

      # inherit the user and group that caddy runs under
      # the key will only readable by the caddy service
      inherit (config.services.caddy) user group;
    };

    # ^^ repeat for `some.host.key`

    services.caddy = {
      virtualHosts."https://some.host".extraConfig = ''
        tls ${config.deployment.keys."some.host.pem".path} ${config.deployment.keys."some.host.key".path}
      '';
    };
  };
}
```
