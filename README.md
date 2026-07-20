![Rust Tests Status](https://img.shields.io/github/actions/workflow/status/forallsys/wire/test.yml?branch=trunk&style=flat-square&label=Rust%20Tests)
![Rust Build & VM Test Status](https://img.shields.io/github/actions/workflow/status/forallsys/wire/build.yml?branch=trunk&style=flat-square&label=Rust%20Build%20%26%20VM%20Test%20Status)
![Documentation Status](https://img.shields.io/github/actions/workflow/status/forallsys/wire/pages.yml?branch=trunk&style=flat-square&label=Documentation)

wire is a tool to deploy NixOS systems. Its usage is inspired by
[colmena](https://colmena.cli.rs/).

Read the [The Tutorial](https://wire.forall.systems/tutorial/overview.html),
[Guides](https://wire.forall.systems/guides/installation.html), or continue
reading this README.

## What is a Hive?

A "Hive" is the attribute set that you pass to `wire.makeHive`. It describes a
collection of NixOS nodes, their nixpkgs, and how to deploy them. It has the
following layout:

```nix
wire.makeHive {
  # `meta` tells wire how to get nixpkgs and how to evaluate nodes.
  meta = {
    # A path or an instance of nixpkgs.
    nixpkgs = <nixpkgs>;
    # Extra specialArgs to pass to each node & defaults.
    specialArgs = { };
    # Override specialArgs per-node.
    nodeSpecialArgs = { };
    # Override nixpkgs per-node.
    nodeNixpkgs = { };
  };

  # `defaults` is a module applied to every node.
  defaults = { ... }: { };

  # `nixosConfigurations` may be passed down from a flake's `self`.
  # Attributes are merged together with nodes of the same name.
  nixosConfigurations = { };

  # Any other attributes are nodes (NixOS modules that describe a system).
  <node-name> = { ... }: { };
}
```

Each node automatically has `defaults` and the wire NixOS module imported, and
is passed `name` (the node's name) and `nodes` (an attribute set of every node
in the hive). See [Write a Hive](https://wire.forall.systems/guides/writing-a-hive.html)
and the [Meta Options reference](https://wire.forall.systems/reference/meta.html).

## Quickstart

### 1. Install wire

Install nix or lix if you don't already have it ([lix.systems](https://lix.systems/install/))
or ([nix.dev](https://nix.dev/install-nix)).
Enable the binary cache before continuing, otherwise you will be compiling
wire from source:

```nix
# configuration.nix
{
  nix.settings = {
    extra-substituters = [
      # ...
      "https://cache.forall.systems"
    ];
    extra-trusted-public-keys = [
      # ...
      "cache.forall.systems:5PmD7QO4MSF8YgyRZtkSGXRDo96H3bybIf2SsQh8ScI="
    ];
  };
}
```

```conf
# /etc/nix/nix.conf
extra-substituters = https://cache.forall.systems
extra-trusted-public-keys = cache.forall.systems:5PmD7QO4MSF8YgyRZtkSGXRDo96H3bybIf2SsQh8ScI=
```

> The `wire` binary and the `wire.makeHive` function are tightly coupled, so
> it is recommended that you use the same version for both. Stick to either a
> tagged version or the `stable` branch which tracks the latest stable tag.

#### With flakes

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    wire.url = "github:forallsys/wire/stable";
    # alternatively, pin a tag:
    # wire.url = "github:forallsys/wire/v1.4.0";
    systems.url = "github:nix-systems/default";
  };
  outputs =
    {
      nixpkgs,
      wire,
      systems,
      ...
    }:
    let
      forAllSystems = nixpkgs.lib.genAttrs (import systems);
    in
    {
      wire = wire.makeHive {
        meta.nixpkgs = import nixpkgs { localSystem = "x86_64-linux"; };
        # Continue to the next section to fill this in.
      };
      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            buildInputs = [ wire.packages.${system}.wire ];
          };
        }
      );
    };
}
```

#### With npins

```sh
npins add github forallsys wire --branch stable
# or pin a tag:
npins add github forallsys wire --at v1.4.0
```

```nix
# shell.nix
let
  sources = import ./npins;
  pkgs = import sources.nixpkgs { };
  wire = import sources.wire;
in
pkgs.mkShell {
  packages = [
    wire.packages.${builtins.currentSystem}.wire
    pkgs.npins
  ];
}
```

### 2. Write a `hive.nix`

```nix
let
  sources = import ./npins;
  wire = import sources.wire;
in
wire.makeHive {
  meta.nixpkgs = import sources.nixpkgs { };

  defaults =
    {
      name,
      nodes,
      pkgs,
      ...
    }:
    {
      imports = [
        ./default-module.nix
        # some-flake.nixosModules.default
      ];
      environment.systemPackages = [ pkgs.vim ];
    };

  node-a =
    {
      name,
      nodes,
      pkgs,
      ...
    }:
    {
      imports = [ ./node-a ]; # hardware-config, etc.
      deployment.target.host = "192.0.2.1";
      deployment.tags = [ "x86" ];
    };

  # ...as many nodes as you'd like.
}
```

### 3. Inspect and deploy

```sh
$ wire show
Node node-a (x86_64-linux):

 > Connection: {root@node-a:22}
 > Build remotely `deployment.buildOnTarget`: false
 > Local apply allowed `deployment.allowLocalDeployment`: true

Summary: 1 total node(s), totalling 0 keys (0 distinct).
Note: Listed connections are tried from Left to Right

$ wire apply switch           # switch every node (≈ nixos-rebuild switch)
$ wire apply switch --on node-a,node-b
$ wire apply keys --on @cloud  # push only secrets to nodes tagged "cloud"
```

`--path` optionally accepts a flakeref: `--path github:foo/bar`,
`--path git+file:///...`, `--path https://.../main.tar.gz`, and `--path ~/my-hive`.

## Apply Goals

`wire apply` accepts a goal, which includes verbs familiar to `nixos-rebuild`
users (`switch`, `boot`, `test`, `dry-activate`) alongside wire-specific verbs:

- `wire apply keys` - push all deployment keys to nodes and do nothing else.
- `wire apply push` - `nix copy` the derivation that produces the node's
  NixOS system to the target.
- `wire apply build` - build the node's NixOS system and ensure the output
  path exists on the node (respecting `deployment.buildOnTarget`).
- `wire apply [switch|boot|test|dry-activate]` - like `nixos-rebuild`.

## Targeting Nodes

```sh
wire apply switch --on node-a,node-b     # literal node names
wire apply --on @cloud                   # all nodes tagged "cloud"
wire apply --on @cloud --on node-5       # mix tags and node names
wire apply --on @cloud @on-prem          # union of tags
echo "node-a node-b" | wire apply --on @other --on -   # read nodes from stdin
```

```nix
node-1.deployment.tags = ["cloud"];
node-2.deployment.tags = ["cloud" "on-prem"];
```

## Secret Management

wire is unopinionated about how you encrypt your secrets. It only handles
pushing and setting up permissions of key files. A key's `source` may be a
literal string (unencrypted), a path (unencrypted, world-readable in the
store), or a command wire runs to evaluate the key (non-interactive, writing
to stdout). Programs that work well include GPG, [sops](https://github.com/getsops/sops),
[Age](https://github.com/FiloSottile/age), and anything that non-interactively
decrypts to stdout.

```nix
node-1 = {
  # Trivial string
  deployment.keys."file.txt".source = ''
    Hello World!
  '';

  # Decrypt with GPG
  deployment.keys."file.txt".source = [
    "gpg" "--decrypt" "${./secrets/file.txt.gpg}"
  ];

  # Ownership (defaults to root:root 0600)
  deployment.keys."file.txt" = {
    source = [ "gpg" "--decrypt" "${./secrets/file.txt.gpg}" ];
    user = "my-user";
    group = "my-group";
  };
};
```

Because wire ships a Rust binary to receive encrypted key data on the remote,
your deploying user must be trusted or you must add wire's binary cache as a
trusted public key:

```nix
{ config, ... }: {
  nix.settings.trusted-users = [ config.deployment.target.user ];
}
```

Please read [Manage Secrets](https://wire.forall.systems/guides/keys.html) for further examples.

## Non-Root Deployments

Wire supports interactive `sudo` prompts! For deployment commands to succeed, the user defined in `deployment.target.user` must be a sudo user authenticated through SSH keys (password-based SSH auth is not supported), and must be listed in `trusted-users` if you deploy secrets. Please read [Use a non-root user](https://wire.forall.systems/guides/non-root-user.html) for more information.

## Building in CI

`wire build` builds nodes locally without pinging or pushing the
result, making it useful for CI. It accepts the same `--on` argument as
`wire apply`, plus a `--partition current/total` option that splits
selected nodes into buckets:

```sh
wire build --partition 1/2
wire build --partition 2/2
```

Please read [Build in CI](https://wire.forall.systems/guides/build-in-ci.html)
for examples.

## Migrating from Colmena

Please read [Migrating to wire - Colmena](https://wire.forall.systems/guides/migrate.html#from-colmena).

## Using alongside `nixos-rebuild`

Please read [Keep Using nixos-rebuild](https://wire.forall.systems/guides/flakes/nixos-rebuild.html).

## Support

Thank you to my GitHub sponsors. You can support development
[here](https://github.com/sponsors/mrshmllow).

![Supporter Graph](./doc/assets/graph.png)

## Development

Please use `nix develop` for access to the development environment and to ensure
your changes are ran against the defined git hooks. For simplicity, you may wish
to use [direnv](https://github.com/direnv/direnv).

`ty check` will download the entire nixpkgs git repo. I know, its stupid. It may
take a very long time to download on first run.

To run cargo commands you'll need to setup a development sqlite file for sqlx.
Just run

```
sqlx database create
sqlx migrate run --source crates/core/src/cache/migrations
```

and it should resolve the issue.
