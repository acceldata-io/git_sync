#!/usr/bin/env bash
set -e
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  rustup default stable
fi

sudo dnf -y install cmake gcc gcc-c++ openssl-devel rpmdevtools git

echo "Installing build dependencies..."
cargo install cargo-generate-rpm cargo-auditable cargo-audit
cargo auditable build --release
cargo generate-rpm

RPM_FILE="$(find ~+ -type f -name "*.rpm")"
if [ -f "$RPM_FILE" ]; then
  echo "The built rpm can be found at $RPM_FILE"
  printf "Run to install:\nsudo dnf install %s\n" "$RPM_FILE"
  echo "You can run 'cargo clean' to clean-up all build artifacts."

else
  echo >&2 "Something went wrong. The .rpm could not be found"
  exit 1
fi
