{
  description = "SigVim — A vim-first Signal desktop client for Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
        };

        # Native build inputs — tools needed at build time.
        nativeBuildInputs = with pkgs; [
          rustToolchain
          pkg-config
          wrapGAppsHook4
          glib # provides glib-compile-resources / gschemas
        ];

        # Library inputs — linked against at build time, present at runtime.
        buildInputs = with pkgs; [
          gtk4
          libadwaita
          glib
          gobject-introspection
          openssl
          sqlite
          dbus
        ];

        # Extra dev-only tools.
        devTools = with pkgs; [
          signal-cli
          dbus
          gdb
          cargo-watch
          cargo-edit
          cargo-outdated
          cargo-nextest
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          inherit nativeBuildInputs buildInputs;
          packages = devTools;

          # GSettings schemas need to be discoverable for libadwaita widgets
          # to resolve their default styles at runtime in the dev shell.
          shellHook = ''
            export XDG_DATA_DIRS="${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}:${pkgs.libadwaita}/share/gsettings-schemas/${pkgs.libadwaita.name}:$XDG_DATA_DIRS"
            export GSETTINGS_SCHEMA_DIR="${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}/glib-2.0/schemas"
            echo "==> SigVim dev shell"
            echo "    cargo run        # build + launch"
            echo "    cargo nextest run # tests"
            echo "    cargo watch -x check"
          '';
        };

        # `nix build` produces a release binary.
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "sigvim";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          inherit nativeBuildInputs buildInputs;
          meta = with pkgs.lib; {
            description = "A vim-first Signal desktop client for Linux";
            homepage = "https://github.com/teddytennant/sigvim";
            license = with licenses; [ mit asl20 ];
            mainProgram = "sigvim";
            platforms = platforms.linux;
          };
        };
      });
}
