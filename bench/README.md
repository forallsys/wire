# Bench

This directory contains a little tool to run hyperfine against wire and colmena, deploying the exact same hive.

The hive can be found in `default.nix`.

Run the test with `nix run .#checks.x86_64-linux.bench.driverInteractive -vvv -L
--show-trace --impure`

Then run `test_script()`

No idea why running the test directly breaks it....

You can adjust the number of nodes in `num-nodes.nix`

The hive has around 20 nodes and 200 keys each. 80% of the keys are pre-activation, 20% post-activation.

| Command                  |        Mean [s] | Min [s] | Max [s] |    Relative |
| :----------------------- | --------------: | ------: | ------: | ----------: |
| `wire@HEAD - flake`      | 89.825 ± 22.941 |  78.190 | 130.831 |        1.00 |
| `wire@stable - flake`    | 133.664 ± 0.303 | 133.219 | 134.044 | 1.49 ± 0.38 |
| `colmena@pinned - flake` | 131.544 ± 1.076 | 130.330 | 133.211 | 1.46 ± 0.37 |
| `wire@stable - hive.nix` | 133.070 ± 0.805 | 132.166 | 134.209 | 1.48 ± 0.38 |
| `wire@HEAD - hive.nix`   | 130.287 ± 1.456 | 128.980 | 132.699 | 1.45 ± 0.37 |
