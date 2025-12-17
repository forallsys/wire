---
comment: true
title: Target Nodes
description: Tags, nodes, and how to target them with wire Tool.
---

# Target Nodes

{{ $frontmatter.description }}

## Targeting Specific Nodes

`wire apply --on` without an `@` prefix interprets as a literal node name. For
example:

```sh
$ wire apply switch --on node-a,node-b
```

Will switch-to-configuration on node a, and node b.

## Reading from Stdin

Passing `--on -` will read whitespace-separated nodes and tags from stdin. This
can be combined with normal `--on` usage.

For example:

```sh
$ echo "node-a node-b" | wire apply --on @other --on -
```

Will apply on `node-a`, `node-b`, and all nodes with the tag `@other`.

## Tag Basics

Nodes can have _tags_, which allows you to easily target multiple, related
nodes for deployment.

```nix:line-numbers{9,13,17,21} [hive.nix]
let
  sources = import ./npins;
  wire = import sources.wire;
in wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  node-1 = {
    # ...
    deployment.tags = ["cloud"];
  };
  node-2 = {
    # ...
    deployment.tags = ["cloud", "virtual"];
  };
  node-3 = {
    # ...
    deployment.tags = ["on-prem"];
  };
  node-4 = {
    # ...
    deployment.tags = ["virtual"];
  };
  node-5 = {
    # Untagged
  };
}
```

To target all nodes with a specific tag, prefix tags with an `@`.
For example, to deploy only nodes with the `cloud` tag, use

```sh
$ wire apply --on @cloud
```

## Further Examples

::: info

Other operations such as an `--ignore` argument are unimplemented as of wire `v0.2.0`.

:::

### Mixing Tags with Node Names

You can mix tags and node names with `--on`:

```sh
$ wire apply --on @cloud --on node-5
```

This will deploy all nodes in `@cloud`, alongside the node `node-5`.

### Targeting Many Tags (Union)

You can specify many tags together:

```sh
$ wire apply --on @cloud @on-prem
```

This is a union between `@cloud` and `@on-prem`.
