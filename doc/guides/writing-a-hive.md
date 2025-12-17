---
comment: true
title: Write a Hive
---

# Write a Hive

## Anatomy of a Hive

A "Hive" is the attribute set that you pass to `wire.makeHive`. It has the
following layout:

```nix
wire.makeHive {
    # `meta`
    # type: attrset
    meta = {
        # `meta.nixpkgs` tells wire how to get nixpkgs.
        # type: "A path or an instance of nixpkgs."
        nixpkgs = <nixpkgs>;

        # `meta.specialArgs` are specialArgs to pass to each node & default
        # type: attrset
        specialArgs = { };

        # `meta.nodeNixpkgs` lets you override nixpkgs per-node.
        # type: attrset of "A path or an instance of nixpkgs."
        nodeNixpkgs: = { };
    };

    # `defaults` is a module applied to every node
    # type: NixOS Module
    defaults = { ... }: { };

    # Any other attributes are nodes.
    <node-name> = { ... }: { };
}
```

### `<node-name>`

Other attributes are NixOs modules that describe a system. They automatically
have `defaults` and the wire NixOS module imported.

They also have the `name` and `nodes` attributes passed to them, `name` being a string of the nodes name, and `nodes` being an attribute set of every node in the hive.

### `meta`

There is more detailed information about `meta` in [the
reference](/reference/meta.html).

### `defaults`

De-duplicate options with default node configuration.

At the top level of a hive wire reserves the `defaults` attribute. It's applied
to every node.

## Example

There is more detailed information the special options for nodes [the
reference](/reference/module.html).

```nix:line-numbers [hive.nix]
{
  meta.nixpkgs = import some-sources-or-inputs.nixpkgs { };

  defaults = {
    # name of the node that defaults is being applied to
    name,
    # attribute set of all nodes
    nodes,
    pkgs,
    ...
  }: {
    import = [
      ./default-module.nix

      # module that is imported for all nodes
      some-flake.nixosModules.default
    ];

    # all nodes should include vim!
    environment.systemPackages [ pkgs.vim ];
  };

  node-a = {
    # name of the node that defaults is being applied to
    name,
    # attribute set of all nodes
    nodes,
    pkgs,
    ...
  }: {
    imports = [
      # import the hardware-config and all your extra stuff
      ./node-a
    ];

    deployment = {
      target.host = "192.0.2.1";
      tags = [ "x86" ];
    };
  };

  # as many nodes as you'd like...

  node-g = {
    # some more config
  };
}
```
