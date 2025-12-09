---
comment: true
title: Use a non-root user
description: Deploy without root permissions with wire.
---

# {{ $frontmatter.title }}

{{ $frontmatter.description }}

## Deploying User Requirements

If your selected deployment user does not fit the following requirements, the
deployment commands will likely fail with an error:

| `deployment.target.user` has/is... | ❌ Will Not Work | 🟧 Deploys w/o Keys | ✅ Deploys w/ Keys |
| :--------------------------------- | :--------------: | :-----------------: | :----------------: |
| In `wheel` (Sudo User)             |        No        |         Yes         |        Yes         |
| Has Non-Interactive SSH Auth       |        -         |         Yes         |        Yes         |
| A Trusted User                     |        -         |         No          |        Yes         |

When using a non-trusted user, `wire apply` will likely fail if the deploying user is
not trusted, see [Manage Secrets - Prerequisites](/guides/keys.html#prerequisites).

- "In `wheel`" here meaning a sudoer, whether it be `root` or not.
- "Non-interactive SSH Auth" here most likely meaning an SSH key, anything that
  does not require keyboard input in the terminal.

To put it simply, wire can currently prompt for your password on `sudo`,
but not `ssh`.

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
deploy-user@node:22 | Authenticate for "sudo /nix/store/.../bin/agent":
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
