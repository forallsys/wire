{
  callPackage,
  wire-small-dev,
  nix,
  nodejs,
  pnpm,
  stdenv,
  mode ? "unstable",
  ...
}:
let
  optionsDoc = callPackage ./options.nix { };
  pkg = builtins.fromJSON (builtins.readFile ./package.json);
in
stdenv.mkDerivation (finalAttrs: {
  inherit (pkg) version;
  pname = pkg.name;
  nativeBuildInputs = [
    wire-small-dev
    nodejs
    pnpm.configHook
    nix
  ];
  src = ./.;
  pnpmDeps = pnpm.fetchDeps {
    inherit (finalAttrs) pname version src;
    fetcherVersion = 1;
    hash = "sha256-ydgb5NCFsYaDbmLjBqu91MqKj/I3TKpNLjOvyP+aY8o=";
  };
  patchPhase = ''
    cat ${optionsDoc} >> ./reference/module.md
    wire inspect --markdown-help > ./reference/cli.md
  '';
  buildPhase = "pnpm run build > build.log 2>&1";
  installPhase = "cp .vitepress/dist -r $out";
  doCheck = true;
  checkPhase = ''
    nix-instantiate --eval --strict ./snippets > /dev/null
  '';
  DEBUG = "*";
  MODE = mode;
})
