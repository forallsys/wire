# Bench

This directory contains a little tool to run hyperfine against wire and colmena, deploying the exact same hive.

The hive can be found in `default.nix`.

Run the test with `nix run .#bench-runner --impure`

The hive has around 20 nodes and 200 keys each. 80% of the keys are pre-activation, 20% post-activation.

| Command          |         Mean [s] | Min [s] | Max [s] |    Relative |
| :--------------- | ---------------: | ------: | ------: | ----------: |
| `colmena@pinned` | 301.977 ± 17.026 | 288.432 | 321.090 | 2.51 ± 0.35 |
| `wire@HEAD`      | 120.123 ± 15.044 | 110.539 | 137.462 |        1.00 |
