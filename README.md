![Rust Tests Status](https://img.shields.io/github/actions/workflow/status/forallsys/wire/test.yml?branch=trunk&style=flat-square&label=Rust%20Tests)
![BuildBot Build & VM Test Status](https://img.shields.io/github/checks-status/forallsys/wire/trunk?style=flat-square&label=BuildBot%20Build%20%26%20VM%20Tests)
![Documentation Status](https://img.shields.io/github/actions/workflow/status/forallsys/wire/pages.yml?branch=trunk&style=flat-square&label=Documentation)

wire is a tool to deploy nixos systems. its usage is inspired by colmena however it is not a fork.

Read the [The Tutorial](https://wire.forall.systems/tutorial/overview.html), [Guides](https://wire.forall.systems/guides/installation.html), or continue reading this readme for development information.

## Development

Please use `nix develop` for access to the development environment and to ensure
your changes are ran against the defined git hooks. For simplicity, you may wish
to use [direnv](https://github.com/direnv/direnv).
