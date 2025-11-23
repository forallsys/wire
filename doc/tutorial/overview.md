---
comment: false
title: wire Tutorial Overview
description: In this tutorial we will create and deploy a wire Hive.
---

# {{ $frontmatter.title }}

wire is a tool to deploy NixOS systems. Its usage is inspired by [colmena](https://colmena.cli.rs/). In many places it's configuration attempts to remain a superset[^1] of colmena, however it is **not** a fork.

[^1]: A lot of your colmena module options will continue to work with wire, but wire has additional ergonomic changes you can take advantage of.

::: warning
wire is alpha software, please use at your own risk.
:::

---

In this tutorial we will create and deploy a wire Hive. Along the way we will
encounter [npins](https://github.com/andir/npins), simple NixOS
configurations, virtual machines, and deployment keys.

You'll need at least 10~ GB of free disk space to complete this tutorial.

<div class="tip custom-block" style="padding-top: 8px">

Ready? Skip to [Nix Setup](./part-one/nix-setup).

</div>

## Why wire?

### Features

| Features                                                      | wire               | Colmena            |
| ------------------------------------------------------------- | ------------------ | ------------------ |
| [Node Tagging](/guides/targeting.html#tag-basics)             | :white_check_mark: | :white_check_mark: |
| [Secret Management](/guides/keys.html)                        | :white_check_mark: | :white_check_mark: |
| [Parallel Deployment](/guides/parallelism.html)               | :white_check_mark: | :white_check_mark: |
| Remote Builds                                                 | :white_check_mark: | :white_check_mark: |
| [Key Services](/guides/keys.html#using-keys-with-services)    | :white_check_mark: | :white_check_mark: |
| [Pipeline Support](/guides/targeting.html#reading-from-stdin) | :white_check_mark: | :x:[^2]            |
| [Non-Root Deployments](/guides/non-root-user)                 | :white_check_mark: | :x:[^3]            |
| `--path` accepts flakerefs                                    | :white_check_mark: | :x:                |
| REPL & Eval expressions                                       | :x:                | :white_check_mark: |
| Adhoc remote command execution[^4]                            | :x:                | :white_check_mark: |

[^2]: You need to write custom nix code to use Colmena hive metadata inside environments like CI pipelines, bash scripting, etc., which requires a knowledge of its internals. Recently it agained the [eval feature](https://colmena.cli.rs/unstable/features/eval.html) which has improved the situation since wire was first started.

[^3]: See https://github.com/zhaofengli/colmena/issues/120

[^4]: wire lacks an equivalent to `colmena exec`.

### Speed

wire is about >2x faster than colmena deploying [identical large
hives](https://github.com/mrshmllow/wire/blob/trunk/bench/run.nix).

| Command          |         Mean [s] | Min [s] | Max [s] |    Relative |
| :--------------- | ---------------: | ------: | ------: | ----------: |
| `colmena@pinned` | 301.977 ± 17.026 | 288.432 | 321.090 | 2.51 ± 0.35 |
| `wire@HEAD`      | 120.123 ± 15.044 | 110.539 | 137.462 |        1.00 |
