{
  perSystem =
    {
      pkgs,
      self',
      ...
    }:
    {
      packages = {
        docs = pkgs.callPackage ./package.nix {
          mode = "stable";
          inherit (self'.packages) wire-small-dev;
        };

        docs-unstable = pkgs.callPackage ./package.nix {
          inherit (self'.packages) wire-small-dev;
        };
      };
    };
}
