---
comment: true
title: Keep Using nixos-rebuild
description: How to combine outputs.nixosConfigurations with outputs.wire
---

# Keep Using nixos-rebuild

{{ $frontmatter.description }}

## An Example

You can provide `makeHive` with your `nixosConfigurations` with the `inherit`
nix keyword. `makeHive` will merge any nodes and nixosConfigurations that share
the same name together.

::: tip
You should include the wire module, which will provide the `deployment` options, even if nixos-rebuild can't directly use them.
:::

::: code-group
<<< @/snippets/getting-started/flake-merged.nix [flake.nix]
:::

Now, if we run `wire show`, you will see that wire only finds
the `nixosConfigurations`-es that also match a node in the hive.
`some-other-host` is not included in the hive unless specified in `makeHive`.

```
$ wire show
Node node-a (x86_64-linux):

 > Connection: {root@node-a:22}
 > Build remotely `deployment.buildOnTarget`: false
 > Local apply allowed `deployment.allowLocalDeployment`: true

Summary: 1 total node(s), totalling 0 keys (0 distinct).
Note: Listed connections are tried from Left to Right

```

This way, you can continue using `nixos-rebuild` and wire at the same time.
