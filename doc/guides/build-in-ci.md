---
comment: true
title: Build in CI
---

# Build in CI

## The `wire build` command <Badge type="tip" text="^1.1.0" />

`wire build` builds nodes locally. It is distinct from
`wire apply build`, as it will not ping or push the result,
making it useful for CI.

It accepts the same `--on` argument as `wire apply` does.

## Partitioning builds

`wire build` accepts a `--partition` option inspired by
[cargo-nextest](https://nexte.st/docs/ci-features/partitioning/), which splits
selected nodes into buckets to be built separately.

It accepts values in the format `--partition current/total`, where 1 ≤ current ≤ total.

For example, these two commands will build the entire hive in two invocations:

```sh
wire build --partition 1/2

# later or synchronously:

wire build --partition 2/2
```

## Example: Build in Github Actions

<<< @/snippets/guides/example-action.yml [.github/workflows/build.yml]
