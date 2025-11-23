![Rust Tests Status](https://img.shields.io/github/actions/workflow/status/mrshmllow/wire/test.yml?branch=trunk&style=flat-square&label=Rust%20Tests)
![BuildBot Build & VM Test Status](https://img.shields.io/github/checks-status/mrshmllow/wire/trunk?style=flat-square&label=BuildBot%20Build%20%26%20VM%20Tests)
![Documentation Status](https://img.shields.io/github/actions/workflow/status/mrshmllow/wire/pages.yml?branch=trunk&style=flat-square&label=Documentation)

wire is a tool to deploy nixos systems. its usage is inspired by colmena however it is not a fork.

Read the [The Tutorial](https://wire.althaea.zone/tutorial/overview.html) , [Guides](https://wire.althaea.zone/guides/installation.html), or continue reading this readme for development information.

## Tree Layout

```
wire
├── wire
│  ├── lib
│  │  └── Rust library containing business logic, consumed by `wire`
│  ├── cli
│  │  └── Rust binary, using `lib`
│  └── key_agent
│     └── Rust binary ran on a target node. receives key file bytes and metadata w/ protobuf over SSH stdin
├── doc
│  └── a [vitepress](https://vitepress.dev/) site
├── runtime
│  └── Nix files used during runtime to evaluate nodes
├── bench
│  └── A little tool to benchmark wire against a large hive
└──tests
   └── Directories used during cargo & NixOS VM testing
```

## Development

Please use `nix develop` for access to the development environment and to ensure
your changes are ran against the defined git hooks. For simplicity, you may wish
to use [direnv](https://github.com/direnv/direnv).

### Testing

#### dhat profiling

```sh
$ just build-dhat
```

#### Testing

```sh
$ cargo test
$ nix flake check
```
