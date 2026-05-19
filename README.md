![Rust Tests Status](https://img.shields.io/github/actions/workflow/status/forallsys/wire/test.yml?branch=trunk&style=flat-square&label=Rust%20Tests)
![Rust Build & VM Test Status](https://img.shields.io/github/actions/workflow/status/forallsys/wire/build.yml?branch=trunk&style=flat-square&label=Rust%20Build%20%26%20VM%20Test%20Status)
![Documentation Status](https://img.shields.io/github/actions/workflow/status/forallsys/wire/pages.yml?branch=trunk&style=flat-square&label=Documentation)

wire is a tool to deploy nixos systems. its usage is inspired by colmena however it is not a fork.

Read the [The Tutorial](https://wire.forall.systems/tutorial/overview.html), [Guides](https://wire.forall.systems/guides/installation.html), or continue reading this readme for development information.

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
