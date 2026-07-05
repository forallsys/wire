{
  nix.settings = {
    extra-substituters = [
      # ...
      "https://cache.forall.systems"
    ];
    extra-trusted-public-keys = [
      # ...
      "cache.forall.systems:5PmD7QO4MSF8YgyRZtkSGXRDo96H3bybIf2SsQh8ScI="
    ];
  };
}
