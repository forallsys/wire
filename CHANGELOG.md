# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - yyyy-mm-dd

In this release garnix has been replaced with our own binary cache as garnix has shutdown.
This is a breaking change to your configuration if you previously
used garnix to download wire.

Please read https://wire.forall.systems/guides/installation.html#binary-cache
for the up to date details!

### Added

- `--print-build-logs` / `-L` argument.
- `nix copy` & `nix build` operations are now manually implemented through a native
  rust nix daemon client if `--experimental-nix-client` is passed.
- Node `target` liveliness is now determined directly by initiating a nix daemon
  handshake if `--experimental-nix-client` is passed.
- Node evaluations are now cached between wire invocations in flake hives,
  invalidating old caches with a connection to the local nix daemon.
- Parsing the output generated from `makeHive` now supports reading the schema version in semver
  format alongside the previous integer-based system. Currently, it still reports `1` to maintain
  backwards compatibility with v1.3.0.
- A list of nodes that are currently deploying, failed, or waiting on a specific
  step are displayed below the status bar.

### Changed

- Changed public cache to `https://cache.forall.systems`, as noted above.
- The pre-activation key stage is now scheduled after the evaluation stage. The
  rationale for this is that while iterating on evaluation errors it was often quite
  annoying to repeatedly enter your password for key deployment.
- Ipv6 addresses are now displayed in a nicer format in "Authenticate for ..."
  prompts.
- Build logs from `-L` are now properly traced and logged alongside the build
  job name.
- Internal representation of Nix store paths now use `snix`'s `StorePath`
  data type.
- Flake evaluation caching now use `snix`'s `StorePath` native digests and name field
  as primary keys instead of full sqlite `text`.
- Wire's cache is now deleted and recreated if migrations fail.
- No longer panics on key agent env var not found.
- Building & Pushing derivation outputs now explicitly operate on the `^out`
  output instead of incorrectly pushing all outputs (`^*`). This results in a
  roughly 12% speed increase in benchmarking.
- Unknown fields in the hive schema are now ignored rather than rejected.
- Cache database now uses `SqliteJournalMode::Wal` & `SqliteSynchronous::Normal`.
- Attempting to reconnect to a rebooting node will now wait far longer before
  giving up.

### Fixed

- Use `AssertPathExists` instead of conditional path existence checks for key services.
- Built paths printed to stdout being clobbered with the status bar.
- Not re-exporting `outputs.makeHive` under `outputs.lib.makeHive`.

## [v1.3.0] - 2026-05-02

### Added

- "Encrypting with Sops" documentation example.
- `--ssh-verbose` / `--sv` argument which increases verbosity of SSH commands.

### Changed

- Under the hood improvements to how status bar updates are handled internally.
- Cargo dependency updates.
- Switched (back) to the https://snix.dev/ `nix_compat` crate for internal nix
  json log parsing.

### Fixed

- Status bar is cleaned every time after execution is completed.
- Fixed garnix docs links in documentation.
- Forces `bash` instead of remote user's potentially unsupported shell. This bug
  was causing strange and hard to diagnose issues.
- Fixed a possible time-of-check to time-of-use bug while setting key permissions.
- `deployment.privilegeEscalationCommand` not being consistently applied.

## [v1.2.0] - 2026-03-18

### Added

- Manpages for `1` & `5`, including subcommands.

### Changed

- The domain for documentation to be `wire.forall.systems`. The previous URL
  will continue to be available but may redirect in the future.
- Refactored node execution to be in two distinct phases, "planning" and
  "execution". Previously, picking what steps would be run was done on the fly
  during execution.
- Cases where there are no keys to deploy, such as having 0 keys or filtered
  keys, the "Key" step will not be planned when it previously would have.
- Changed non-interactive SSH executed commands to use `BatchMode=yes` instead
  of using `PasswordAuthentication=no` and `KbdInteractiveAuthentication=no`.

### Fixed

- Fix a bug where key permissions where being printed in decimal format instead
  of octal.
- `wire inspect names` without `--json` will now correctly output names as a
  newline separated string instead of always as a json list.
- Fix a bug where errors encountered while reading nodes from stdin where
  silently ignored

### Removed

- Remove "Error Codes" documentation page & links.

## [v1.1.1] - 2025-01-05

### Fixed

- Fix a bug where wire was attempting to SSH to the local machine when `buildOnTarget` &
  `allowLocalDeployment` where true.

## [v1.1.0] - 2025-12-31

### Added

- Add a `--substitute-on-destination` argument.
- Add the `meta.nodeSpecialArgs` meta option.
- Add `wire build`, a new command to build nodes offline.
  It is distinct from `wire apply build`, as it will not ping
  or push the result, making it useful for CI.

### Changed

- Build store paths will be output to stdout

### Fixed

- Fix invalidated caches not actually returning `None`.

## [v1.0.0] - 2025-12-17

### Added

- SIGINT signal handling.

### Changed

- Invalidate caches that reference garbage collected paths.

### Fixed

- Fix key filtering logic.

## [v1.0.0-beta.0] - 2025-12-02

### Added

- Implement `meta.nodeNixpkgs`.
- Add caching of hive evaluation for flakes.

### Changed

- Run tests against 25.11.

## [v1.0.0-alpha.1] - 2025-11-24

### Added

- Add `--handle-unreachable`. You can use `--handle-unreachable ignore` to
  ignore unreachable nodes in the status of the deployment.
- Add a basic progress bar.

### Changed

- Revert "Wire will now attempt to use SSH ControlMaster by default.".
- Change the `show` subcommand to look nicer now.
- Change the `build` step to always build remotely when the node is
  going to be applied locally.

## [v1.0.0-alpha.0] - 2025-10-22

### Added

- Add `--ssh-accept-host` argument.
- Add `--on -` syntax to the `--on` argument.
  Passing `-` will now read additional apply targets from stdin.
- Add `{key.name}-key.{path,service}` systemd units.
- Added `--flake` argument as an alias for `--path`.
- A terminal bell will be output if a sudo / ssh prompt is ever printed.
- Added a real tutorial, and separated many how-to guides.
  The tutorial leads the user through creating and deploying a wire Hive.
- Add `config.nixpkgs.flake.source` by default if `meta.nixpkgs` ends
  with `-source` at priority 1000 (default).

### Fixed

- Fix bug where `--non-interactive` was inversed.
- Fix a bug where `./result` links where being created.
- Fix passing `sources.nixpkgs` directly from npins to `meta.nixpkgs`.
- Fix nodes that will be applied locally running the `push` and `cleanup`
  steps.

### Changed

- Improve logging from interactive commands (absence of `--non-interactive`).
- Changed `--path` argument to support flakerefs (`github:foo/bar`,
  `git+file:///...`, `https://.../main.tar.gz`, etc).
- Changed SSH arguments to use ControlMaster by default.
- Compile-out logs with level `tracing_level::TRACE` in release builds.
- Improve aata integrity of keys.
- Unknown SSH keys will be immediately rejected unless `--ssh-accept-host` is passed.
- Changed evaluation to be ran in parallel with other steps until
  the .drv is required.

## [0.5.0] - 2025-09-18

### Added

- Added `--reboot`. wire will wait for the node to reconnect after rebooting.
  wire will refuse to reboot localhost. Keys post-activation will be applied
  after rebooting!
- Most errors now have error codes and documentation links.
- Added the global flag `--non-interactive`.
- wire now creates its own PTY to interface with openssh's PTY to allow for
  interactive sudo authentication on both remote and local targets.

  Using a wheel user as `deployment.target.user` is no longer necessary
  (if you like entering your password a lot).

  A non-wheel user combined with `--non-interactive` will likely fail.

- Added `deployment.keys.environment` to give key commands environment variables.

### Changed

- `wire inspect/show --json` will no longer use a pretty print.
- wire will now wait for the node to reconnect if activation failed (excluding
  dry-activate).
- Nix logs with the `Talkative` and `Chatty` level have been moved to
  `tracing_level::TRACE`.
- Error messages have been greatly improved.

### Fixed

- Some bugs to do with step execution were fixed.

## [0.4.0] - 2025-07-10

### Added

- Nodes may now fail without stopping the entire hive from continuing. A summary
  of errors will be presented at the end of the apply process.
- wire will now ping the node before it proceeds executing.
- wire will now properly respect `deployment.target.hosts`.
- wire will now attempt each target host in order until a valid one is found.

### Changed

- wire now directly evaluates your hive instead of shipping extra nix code along with its binary.
  You must now use `outputs.makeHive { ... }` instead of a raw attribute.
  This can be obtained with npins or a flake input.
- The expected flake output name has changed from `outputs.colmena` to `outputs.wire`.

## [0.3.0] - 2025-06-20

### Added

- Run tests against `unstable` and `25.05` by @mrshmllow in https://github.com/wires-org/wire/pull/176.

### Changed

- Dependency Updates.
- wire now compiles and includes key agents for multiple architectures, currently only linux.
- There is a new package output, `wire-small`, for testing purposes.
  It only compiles the key agent for the host that builds `wire-small`.
- `--no-progress` now defaults to true if stdin does not refer to a tty (unix pipelines, in CI).
- Added an error for the internal hive evaluation parse failure.
- The `inspect` command now has `show` as an alias.
- Remove `log` command as there are currently no plans to implement the feature
- The `completions` command is now hidden from the help page

### Fixed

- A non-existent key owner user/group would not default to gid/uid `0`.
- Keys can now be deployed to localhost.

## [0.2.0] - 2025-04-21

### Added

- Getting Started Guide by @mrshmllow.
- Web documentation for various features by @mrshmllow.
- Initial NixOS VM Testing Framework by @itslychee in https://github.com/wires-org/wire/pull/93.

### Changed

- `runtime/evaluate.nix`: force system to be null by @itslychee in https://github.com/wires-org/wire/pull/84.

> [!IMPORTANT]  
> You will have to update your nodes to include `nixpkgs.hostPlatform = "<ARCH>";`

- GH Workflows, Formatting, and other DevOps yak shaving.
- Issue Templates.
- Cargo Dependency Updates.
- `doc/` Dependency Updates.
- `flake.nix` Input Updates.

### Fixed

- Keys with a path source will now be correctly parsed as `path` instead
  of `string` by @mrshmllow in https://github.com/wires-org/wire/pull/131.
- `deployment.keys.<name>.destDir` will be automatically created if it
  does not exist. Nothing about it other than existence is guaranteed. By
  @mrshmllow in https://github.com/wires-org/wire/pull/131.
