---
comment: true
title: Use a non-root user
description: Deploy without root permissions with wire.
---

# Use a non-root user

{{ $frontmatter.description }}

## Deploying User Requirements

For deployment commands to succeed, the user defined in `deployment.target.user` must meet the following criteria:

1. Essential Config

- **Sudo Access**: The user must be `wheel` (A sudo user)
- **SSH Key Authentication**: The user must be authenticated through SSH keys,
  and password-based SSH auth is not supported.

  **Why?** Wire can prompt you for your `sudo` password, but not your `ssh` password.

2. Deploying with Secrets

- **Trusted User**: The user must be listed in the `trusted-users` nix config.

  If the user is not trusted, wire will fail in the key deployment stage.

For setting up a trusted user, see [Manage Secrets - Prerequisites](/guides/keys.html#prerequisites).

## Changing the user

By default, the target is set to root:

```nix
{
  deployment.target.user = "root";
}
```

But it can be any user you want so long as it fits the requirements above.

```nix
{
  deployment.target.user = "root"; # [!code --]
  deployment.target.user = "deploy-user"; # [!code ++]
}
```

After this change, wire will prompt you for sudo authentication, and tell you
the exact command wire wants privileged:

```sh{6}
$ wire apply keys --on media
 INFO eval_hive: evaluating hive Flake("/path/to/hive")
...
 INFO media | step="Upload key @ NoFilter" progress="3/4"
deploy-user@node:22 | Authenticate for "sudo /nix/store/.../bin/key_agent":
[sudo] password for deploy-user:
```

## Using alternative privilege escalation

You may change the privilege escalation command with the
[deployment.privilegeEscalationCommand](/reference/module.html#deployment-privilegeescalationcommand)
option.

For example, doas:

```nix
{
  deployment.privilegeEscalationCommand = [
    "sudo" # [!code --]
    "--" # [!code --]
    "doas" # [!code ++]
  ];
}
```

Note that `run0` requires the `--pipe` argument:

```nix
{
  deployment.privilegeEscalationCommand = [
    "run0"
    "--pipe" # [!code ++]
    "--"
  ];
}
```
