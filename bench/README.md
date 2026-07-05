# Bench

This directory contains a little tool to run hyperfine against wire and colmena, deploying the exact same hive.

The hive can be found in `default.nix`.

Run the test with `nix run .#checks.x86_64-linux.bench.driverInteractive -vvv -L
--show-trace --impure`

Then run `test_script()`

No idea why running the test directly breaks it....

You can adjust the number of nodes in `num-nodes.nix`

The hive has around 20 nodes and 200 keys each. 80% of the keys are pre-activation, 20% post-activation.

| Command                                         |        Mean [s] | Min [s] | Max [s] |     Relative |
| :---------------------------------------------- | --------------: | ------: | ------: | -----------: |
| `wire@HEAD - flake`                             |   4.804 ± 0.081 |   4.710 |   4.886 |  1.04 ± 0.02 |
| `wire@HEAD & --experimental-nix-client - flake` |   4.636 ± 0.033 |   4.607 |   4.693 |         1.00 |
| `colmena@pinned - flake`                        | 122.410 ± 1.828 | 120.143 | 125.158 | 26.40 ± 0.44 |
| `wire@HEAD - hive.nix`                          | 119.883 ± 1.115 | 118.651 | 121.429 | 25.86 ± 0.30 |
