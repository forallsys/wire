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
| `wire@HEAD - flake`                             |   5.467 ± 0.061 |   5.406 |   5.561 |  1.15 ± 0.02 |
| `wire@HEAD & --experimental-nix-client - flake` |   4.773 ± 0.083 |   4.693 |   4.870 |         1.00 |
| `colmena@pinned - flake`                        | 125.852 ± 0.528 | 125.242 | 126.404 | 26.37 ± 0.47 |
| `wire@HEAD - hive.nix`                          | 122.832 ± 1.216 | 121.555 | 124.179 | 25.73 ± 0.52 |
