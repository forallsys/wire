let
  sources = import ./npins;
  wire = import sources.wire;
in
  wire.makeHive {
    meta.nixpkgs = import sources.nixpkgs {};

    node-1 = {lib, ...}: let
      mkSops = key: [
        "sops"
        "-d"
        "--extract"
        (lib.concatMapStrings (segment: ''["${segment}"]'') key)
        "${./secrets.yaml}"
      ];
    in {
      deployment.key = {
        "some_secret.txt" = {
          source = mkSops [
            "hive"
            "some_secret"
          ];
        };

        "another_secret.txt" = {
          source = mkSops [
            "something"
            "another_secret"
          ];
        };
      };
    };
  }
