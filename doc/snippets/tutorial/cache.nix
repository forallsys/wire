{
  nix.settings = {
    substituters = [
      # ...
      "https://cache.garnix.io?priority=3"
    ];
    trusted-public-keys = [
      # ...
      "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
    ];
  };
}
