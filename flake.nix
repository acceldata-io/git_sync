{
  description = "Native build & dev shell for git_sync with Rust 1.88.0 (musl tools available on Linux)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        normalizedRustTarget = target:
          if target == "arm64-apple-darwin" then "aarch64-apple-darwin" else target;

        nativeTarget = normalizedRustTarget pkgs.stdenv.hostPlatform.config;

        muslTarget = "x86_64-unknown-linux-musl";
        extraRustTargets = pkgs.lib.optional pkgs.stdenv.hostPlatform.isLinux muslTarget;
        extraDevInputs = pkgs.lib.optional pkgs.stdenv.hostPlatform.isLinux pkgs.musl;

        rustToolchain = pkgs.rust-bin.stable."1.88.0".default.override {
          extensions = [ "rust-src" "rust-std" ];
          targets = [ nativeTarget ] ++ extraRustTargets;
        };

        envVars = {
          SOURCE_DATE_EPOCH = "315532800";
          RUSTFLAGS = "--remap-path-prefix=$(pwd)=.";
        };

        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        version = cargoToml.package.version;

        commonBuildInputs = [
          rustToolchain
          pkgs.pkg-config
          pkgs.openssl
          pkgs.openssl.dev
          pkgs.cargo-audit
          pkgs.cargo-auditable
          pkgs.git
        ] ++ extraDevInputs;

      in {
        packages = rec {
          git_sync = pkgs.rustPlatform.buildRustPackage {
            pname = "git_sync";
            version = version;
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };
            buildInputs = commonBuildInputs;
            buildPhase = ''
              export SOURCE_DATE_EPOCH=${envVars.SOURCE_DATE_EPOCH}
              export RUSTFLAGS="${envVars.RUSTFLAGS}"
              cargo auditable build --release
            '';
            installPhase = ''
              mkdir -p $out/bin
              cp target/release/git_sync $out/bin/
            '';
            doCheck = true;
          };
          default = git_sync;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = commonBuildInputs;
          shellHook = ''
            export SOURCE_DATE_EPOCH=${envVars.SOURCE_DATE_EPOCH}
            export RUSTFLAGS="${envVars.RUSTFLAGS}"
            if [[ "${pkgs.stdenv.hostPlatform.system}" == *-linux ]]; then
              echo "You can also build for musl with: cargo auditable build --release --target ${muslTarget}"
            fi
            echo "Native Rust toolchain ready for ${nativeTarget}."
            echo "Build native with: cargo auditable build --release"
          '';
        };
      }
    );
}
