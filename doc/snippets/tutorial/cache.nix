{
  nix.settings = {
    substituters = [
      # ...
      "https://cache.forall.systems"
    ];
    trusted-public-keys = [
      # ...
      # TODO: update
      "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
    ];
  };
}
