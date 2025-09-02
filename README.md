# git_sync

**git_sync** is a Rust tool designed to simplify and automate management of Git repositories on Github. 

---

## Features

- Sync changes between source and destination repositories.
- Sync tags from upstream to your forked repository.
- Manage branches.
- Manage tags.
- Create releases with automatically generated release notes.
- Create and merge pull requests, where possible.
- Run various sanity checks for a repository.
- Run all of the above for all configured repositories 


---

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install)
- Git installed and available in your PATH

### Build from Source

```bash
git clone https://github.com/JeffreySmith/git_sync.git
cd git_sync
cargo build --release
```
The compiled binary can be found at `target/release/git_sync`.

Make sure to build with `--release`. This will drastically speed up the binary.

#### Build a static linux compatible binary

##### Prerequisites
- The `x86_64-unknown-linux-musl` target, which can be installed by running `rustup target add x86_64-unknown-linux-musl`
- A compiler for `x86_64-unknown-linux-musl`. On MacOS, this can be installed through homebrew: `brew install filosottile/musl-cross/musl-cross`. For linux, if the toolchain isn't available in your package manager, you can download `x86_64-linux-musl-cross.tgz` from [musl.cc](https://musl.cc/) (unofficial) or build your own using [musl-cross-make](https://github.com/richfelker/musl-cross-make/)

#### Building
With both the rust and c toolchain installed, you can run `cargo build --release --target x86_64-unknown-linux-musl`. This will produce a completely static binary with no external dependencies. 

This can be verified by running ldd on the binary:
```bash
[user@host release]$ ldd git_sync
        statically linked
```
When building this statically using the above toolchain, you will find the binary at `target/x86_64-unknown-linux-musl/release/git_sync`.

### Configuration
Run `git_sync config --file ~/.config/git-manage.toml` to create an initial configuration. 

The important things to add to this configuration file are as follows:

- Your github api token
- Your repositories in their correct category (public, private, or fork). Forks should be anything that has a parent repository

## Compatibility
This has been verified to run on both Redhat 7 and MacOS 15 meaning that it likely works on just about any unix os.

## Generating documentation and shell completion
To generate new versions of the manpages, you can run `git_sync generate --kind man`. 

To generate shell completion for bash, fish, or zsh, you can run `git_sync generate --kind [shell]` and then copying the output file them into your shell's completions directory.
