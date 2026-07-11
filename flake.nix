{
  description = "open-media — watch movies, series & anime from the terminal (Real-Debrid + P2P → mpv)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  # Prebuilt closures are pushed to the grok-insider cachix cache by CI, so consumers
  # never compile open-media (the rustls/aws-lc + librqbit build is heavy).
  nixConfig = {
    extra-substituters = [
      "https://grok-insider.cachix.org"
      "https://nix-community.cachix.org"
    ];
    extra-trusted-public-keys = [
      "grok-insider.cachix.org-1:ZxLVOxJ1CjdY3vQl1I99qCtwNZwIU4+/QwqSvntB/5w="
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    ];
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      # The `open-media` binary (crate open-media-cli). One output; `default` aliases it.
      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          lib = nixpkgs.lib;
          version = (lib.importTOML ./Cargo.toml).workspace.package.version;

          # open-media-subs depends on open-subtitle via git (not crates.io).
          # importCargoLock needs a NAR hash per git-sourced package name/version.
          # All open-subtitle-* crates share one monorepo rev, so they share one hash.
          openSubtitleGitHash = "sha256-aWlWxXYfrabRRX+f94FvdOgQjf0/4qn3NN05avTUVdk=";

          open-media = pkgs.rustPlatform.buildRustPackage {
            pname = "open-media";
            inherit version;
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
              outputHashes = {
                "open-subtitle-compose-0.0.1" = openSubtitleGitHash;
                "open-subtitle-config-0.0.1" = openSubtitleGitHash;
                "open-subtitle-core-0.0.1" = openSubtitleGitHash;
                "open-subtitle-engine-0.0.1" = openSubtitleGitHash;
                "open-subtitle-identify-0.0.1" = openSubtitleGitHash;
                "open-subtitle-process-0.0.1" = openSubtitleGitHash;
                "open-subtitle-providers-0.0.1" = openSubtitleGitHash;
                "open-subtitle-sync-0.0.1" = openSubtitleGitHash;
                "open-subtitle-transcribe-0.0.1" = openSubtitleGitHash;
                "open-subtitle-translate-0.0.1" = openSubtitleGitHash;
              };
            };

            # Build only the binary crate (and its deps), not the whole workspace.
            cargoBuildFlags = [ "-p" "open-media-cli" ];

            # Native build glue:
            #   - cmake + bindgenHook: aws-lc-sys (rustls' default crypto backend)
            #     needs CMake and libclang/bindgen.
            #   - cc (from stdenv): ring + bundled SQLite (rusqlite "bundled").
            # No OpenSSL and no system SQLite: rustls + vendored sqlite. TLS roots
            # come from webpki-roots, so no ca-certificates dependency at runtime.
            nativeBuildInputs = with pkgs; [
              pkg-config
              cmake
              rustPlatform.bindgenHook
            ];

            # Tests are hermetic (wiremock) and run in CI's `rust` job; skipping
            # them here keeps the package build lean (no wiremock compile).
            doCheck = false;

            meta = {
              description = "Terminal media app: TMDB/AniList → Torrentio/nyaa → Real-Debrid/P2P → mpv";
              homepage = "https://github.com/grok-insider/open-media";
              license = lib.licenses.mit;
              mainProgram = "open-media";
              platforms = systems;
            };
          };
        in
        {
          inherit open-media;
          default = open-media;
        });

      # Home Manager module: installs the `open-media` binary (prebuilt from the cache).
      #
      # NOTE: open-media's config (`~/.config/open-media/config.toml`) holds API
      # tokens (TMDB/Real-Debrid/AniList), so it is intentionally NOT managed
      # here — secrets must never enter the Nix store. Configure it at runtime
      # with `open-media init` / `open-media config set key=value`.
      #
      # Runtime dependency: an external player on PATH (mpv recommended; vlc
      # supported). It is not bundled — the host's own mpv is used.
      homeManagerModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.programs.open-media;
          pkgsFor = self.packages.${pkgs.stdenv.hostPlatform.system};
        in
        {
          options.programs.open-media = {
            enable = lib.mkEnableOption "open-media terminal media app";
            package = lib.mkOption {
              type = lib.types.package;
              default = pkgsFor.default;
              defaultText = lib.literalExpression "open-media.packages.\${system}.default";
              description = "The open-media package providing the `open-media` binary.";
            };
          };
          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];
          };
        };

      # Dev shell: the Rust toolchain plus the native build glue (cmake + libclang
      # for aws-lc-sys, cc for ring/sqlite). `mpv` is added for running the player
      # end-to-end during development.
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            name = "open-media-dev";
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer
              pkg-config
              cmake
              clang
              llvmPackages.libclang
              mpv
            ];
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          };
        });
    };
}
