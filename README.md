# git_sync

**git_sync** is a Rust tool designed to simplify and automate management of Git repositories on Github. 

---

## Features

- Sync changes between a fork and its parent repository.
- Sync tags from upstream to your forked repository.
- Manage branches.
- Manage tags.
- Create releases with automatically generated release notes.
- Create and then merge pull requests, where possible.
- Run various sanity checks for a repository.
- Backup a repository, including backing up to AWS (in progress).
- Send notifications to Slack, if Slack support is enabled (default feature).
- Collect metadata about your repository and save it to a database (TODO).
- Run all of the above for all configured repositories.

---

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install). You must have at least Rust 1.86.0 installed.
- [Git](https://git-scm.com/) installed and available in your PATH

### Build from Source

```bash
git clone https://github.com/JeffreySmith/git_sync.git
cd git_sync
cargo build --release
```
The compiled binary can be found at `target/release/git_sync`.

Make sure to build with `--release`. This will drastically speed up the binary. LTO is automatically enabled for all release builds, which will mean the final binary takes a while to build (about 2-3 minutes), but it does drastically improve performance.

#### Build with optional features
There are a few optional features that can be enabled or disabled at build time. These features can be found under the "[features]" header in Cargo.toml. The default features are "aws" and "slack", which respectively add support for backing up to S3, and sending messages over slack.

You can disable default features by adding `--no-default-features` to the cargo build command; you can enable specific features by adding `--features feature1,feature2` etc to the build command. If you want all optional features enabled, you can also add `--all-features` to the cargo build command. Adding all features will increase the number of libraries that are pulled in and also increase build time slightly.

If you don't enable an optional features at build time, it will not be available at runtime.

At the moment, SQL support is not available despite the feature being in Cargo.toml so there's no reason to enable it. This will change in the future.

#### Setting up the environment with [nix](https://github.com/NixOS/nix) (Optional)
In this repository there is a file, `flake.nix`, which can be used to automatically set up an environment with all of the prerequisites installed. You must already have nix [installed](https://nix.dev/install-nix) on your machine, and then enable an 'experimental' feature to enable flake usage. This can be done by running:
```shell
mkdir -p ~/.config/nix/
echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf

```

Then, from the root of the repository, run `nix develop` and all required tools will be downloaded. If you running this on Linux, this will also setup cross-compilation for musl to build a completely static binary.
#### Build a static Linux compatible binary

##### Prerequisites
- The `x86_64-unknown-linux-musl` target, which can be installed by running `rustup target add x86_64-unknown-linux-musl`
- A compiler for `x86_64-unknown-linux-musl`. On MacOS, this can be installed through homebrew: `brew install filosottile/musl-cross/musl-cross`. For linux, if the toolchain isn't available in your package manager, you can download `x86_64-linux-musl-cross.tgz` from [musl.cc](https://musl.cc/) (unofficial) or build your own using [musl-cross-make](https://github.com/richfelker/musl-cross-make/)

#### Building
With both the rust and the musl c toolchain installed, you can run `cargo build --release --target x86_64-unknown-linux-musl`. This will produce a completely static binary with no external dependencies. 

This can be verified by running ldd on the binary:
```bash
[user@host release]$ ldd git_sync
        statically linked
```
When building this statically using the above toolchain, you will find the binary at `target/x86_64-unknown-linux-musl/release/git_sync`.

##### Security and CVE notes
There are two things that can be done to ensure there are no known CVEs in the resulting binary:

1. Use `cargo-audit` to check for known vulnerabilities in the rust dependencies. This can be installed by running `cargo install cargo-audit` and then run by executing `cargo audit` in the root of the git_sync repository.
2. Use [cargo-auditable](https://github.com/rust-secure-code/cargo-auditable) to ensure that information about all of the dependencies is included in the binary. This can be installed by running `cargo install cargo-auditable` and then any time you would run `cargo build --release`, run instead `cargo auditable build --release`. This will ensure that information about all of the dependencies is included in the binary; this information can be used to detect CVEs by tools such as
    - [cargo-audit](https://crates.io/crates/cargo-audit)
    - [trivy](https://github.com/aquasecurity/trivy)
    - [grype](https://github.com/anchore/grype)
    - [osv-scanner](https://github.com/google/osv-scanner/)

To check for CVEs with Trivy, you can run the following command at the root of the source code:
```shell
trivy fs --scanners vuln,secret,misconfig,license --license-full --skip-dirs target/ .
```



### Configuration
Run `git_sync config --file ~/.config/git-manage.toml` to create an initial configuration. 

The important things to add to this configuration file are as follows:

- Your github api token
- Your repositories in their correct category (public, private, or fork). Forks should be anything that has a parent repository
- Your slack webhook url, if you have enabled Slack integration

Certain commands require that you have git installed and available in your PATH.
That includes the following:
1. `git_sync backup`
2. `git_sync tag sync --annotated `

TODO
~~If you have a forked repository on Github that does not have a configured parent, you can put it into the fork_with_workaround map, where "forked repo" = "actual upstream repo". Ex: `fork_with_workaround = {"https://github.com/my-org/livy" = "https://github.com/apache/incubator-livy"}`. This will require that you have write access to your forked repository, and git set up correctly on your machine.~~

## Getting help
All commands and subcommand have a `--help` flag that will give you information about the various flags, including valid options for each flag.

Rust documentation can be generated by running `sh ./generate_docs.sh`, however, you can also view the documentation online [hosted on Github](https://jeffreysmith.github.io/git_sync/git_sync/)

## Compatibility
This has been verified to run on both Redhat 7 and MacOS 15 meaning that it likely works on just about any unix os. It will likely not work on Windows since this tool expects a unix environment.

## Generating documentation and shell completion
To generate new versions of the manpages, you can run `git_sync generate --kind man`. 

To generate shell completion for bash, fish, or zsh, you can run `git_sync generate --kind [shell]` and then copying the output file them into your shell's completions directory.

The `generate` command will not show up during general usage, but it can always be run by specifying it directly.
