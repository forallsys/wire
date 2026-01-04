# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - yyyy-mm-dd

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
