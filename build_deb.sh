#!/usr/bin/env bash
set -e
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  rustup default stable
fi

sudo apt install -y cmake gcc g++ libssl-dev git dpkg dpkg-dev liblzma-dev

echo "Installing build dependencies..."
cargo install cargo-deb cargo-auditable cargo-audit
cargo auditable deb

DEB_FILE="$(find ~+ -type f -name "*.deb")"
if [ -f "$DEB_FILE" ]; then
  echo "The built deb can be found at $DEB_FILE"
  printf "Run to install:\nsudo dpkg -i %s\n" "$DEB_FILE"
  echo "You can run 'cargo clean' to clean-up all build artifacts."
else
  echo >&2 "Something went wrong. The .deb could not be found"
  exit 1
fi
