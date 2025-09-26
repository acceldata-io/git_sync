# git_sync

**git_sync** is a tool designed to simplify and automate management of Git repositories on Github. 

---

## Features

- Sync changes between a fork and its parent repository.
- Sync tags from upstream to your forked repository.
- Create/delete branches.
- Create/delete tags.
- Create releases with automatically generated release notes.
- Create and automatically merge pull requests, where possible.
- Run various checks for a repository.
- Backup a repository, including backing up to AWS.
- Send notifications to Slack, if Slack support is enabled (default feature).
- Run all of the above for all configured repositories.

---

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install). You will need at least Rust 1.86 installed. This has been tested with Rust 1.88 and 1.90.
- Either [clang](https://clang.llvm.org/) or [gcc](https://gcc.gnu.org/) installed to compile the code since several dependencies require a C compiler to build. Using [musl](https://musl.libc.org/) and [gcc](https://gcc.gnu.org/) to build a static binary is covered in a [later section](#building-with-musl-for-a-completely-static-binary).

### Runtime dependency

- [Git](https://git-scm.com/) installed and available in your PATH. You must have read and write access to repositories for certain commands to work, such as syncing annotated tags and making backups. Most operations use the Github REST API or GraphQL API.

### Build from Source

```bash
$ git clone https://github.com/JeffreySmith/git_sync.git
$ cd git_sync
$ cargo build --release
```

The compiled binary can be found at `target/release/git_sync`.

Make sure to build with `--release`; this will drastically speed up the binary. Link time optimization is automatically enabled for all release builds which will mean the final binary takes a while to build (about 2-3 minutes), but it does drastically improve performance when deserializing JSON, particularly for larger repositories.

#### Build with optional features
There are a few optional features that can be enabled or disabled at build time. These features can be found under the "[features]" header in Cargo.toml. The default features are "aws" and "slack", which respectively add support for backing up to S3, and sending messages over slack.

You can disable default features by adding `--no-default-features` to the cargo build command; you can enable specific features by adding `--features feature1,feature2` etc to the build command. If you want all optional features enabled, you can also add `--all-features` to the cargo build command. Adding all features will increase the number of libraries that are pulled in and also increase build time slightly.

If optional features are disabled at build time, they will not be available at runtime.

#### Setting up the build environment with [Nix](https://github.com/NixOS/nix) (Optional)
In this repository there is a file, `flake.nix`, which can be used to automatically set up an environment with all of the prerequisites installed. You must already have nix [installed](https://nix.dev/install-nix) on your machine, and then enable an 'experimental' feature to enable nix flakes. This can be done by running:
```bash
$ mkdir -p ~/.config/nix/
$ echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf

```
Then, from the root of the repository, run `nix develop` and all required tools will be downloaded. If you running this on Linux, this will also setup cross-compilation for musl to build a completely static binary. 
#### Build a static Linux compatible binary

##### Prerequisites
- The `x86_64-unknown-linux-musl` target, which can be installed by running `rustup target add x86_64-unknown-linux-musl`
- A compiler for `x86_64-unknown-linux-musl`. 
  - On MacOS, this can be installed through homebrew: `brew install filosottile/musl-cross/musl-cross` 
  - On Linux, if the toolchain isn't available in your package manager, you can download `x86_64-linux-musl-cross.tgz` from [musl.cc](https://musl.cc/) (unofficial) or build your own using [musl-cross-make](https://github.com/richfelker/musl-cross-make/)

#### Building with Musl for a completely static binary
With both the rust and the musl c toolchain installed, you can run `cargo build --release --target x86_64-unknown-linux-musl`. This will produce a completely static binary with no external dependencies. 

This can be verified by running ldd on the binary:
```bash
$ ldd git_sync
        statically linked
```
When building this statically using the above toolchain, you will find the binary at `target/x86_64-unknown-linux-musl/release/git_sync`. This binary can only be run on Linux.

#### Security and CVE notes
There are two things that can be done to ensure there are no known CVEs in the resulting binary:

1. Use `cargo-audit` to check for known vulnerabilities in the rust dependencies. This can be installed by running `cargo install cargo-audit` and then run by executing `cargo audit` in the root of the git_sync repository.
2. Use [cargo-auditable](https://github.com/rust-secure-code/cargo-auditable) to ensure that information about all of the dependencies is included in the binary. This can be installed by running `cargo install cargo-auditable` and then any time you would run `cargo build --release`, run instead `cargo auditable build --release`. This will ensure that information about all of the dependencies is included in the binary; this information can be used to detect CVEs by tools such as
    - [cargo-audit](https://crates.io/crates/cargo-audit)
    - [trivy](https://github.com/aquasecurity/trivy)
    - [grype](https://github.com/anchore/grype)
    - [osv-scanner](https://github.com/google/osv-scanner/)

Using `cargo auditable build --release` is the recommended way of building this application.

To check for CVEs in the source code locally with Trivy, you can run the following command at the root of the source code:
```bash
trivy fs --scanners vuln,secret,misconfig,license --license-full --skip-dirs target/doc,target/debug,target/release/build,target/release/build/deps .
```

This command will also scan to make sure that there aren't any issues with licenses, or secrets being stored in the source code.

You can scan the binary with Trivy if it's in a docker image:
```bash
$ docker build -t audit-git-sync -f - . <<EOF
FROM scratch
COPY target/release/git_sync .
EOF
$ trivy image --scanners vuln audit-git-sync
trivy image --scanners vuln audit-git-sync
2025-09-17T12:35:31-04:00       INFO    [vuln] Vulnerability scanning is enabled
2025-09-17T12:35:31-04:00       INFO    Number of language-specific files       num=1
2025-09-17T12:35:31-04:00       INFO    [rustbinary] Detecting vulnerabilities...

Report Summary

┌──────────┬────────────┬─────────────────┐
│  Target  │    Type    │ Vulnerabilities │
├──────────┼────────────┼─────────────────┤
│ git_sync │ rustbinary │        0        │
└──────────┴────────────┴─────────────────┘
Legend:
- '-': Not scanned
- '0': Clean (no security findings detected)
```

The above example was taken from Trivy [release notes for v0.31.0](https://github.com/aquasecurity/trivy/discussions/2716)

You can also check the Github repo for CVEs by running `trivy repo --branch main https://github.com/JeffreySmith/git_sync`.

To use `cargo-audit`, simply run `cargo audit` in the root of the source code tree. If instead you want to check the binary if it's been compiled with `cargo-auditable`, you can run: 
```bash
cargo-audit bin target/release/git_sync
```

This will tell you how many CVEs effect the dependencies the binary is actually using, rather than everything listed in Cargo.toml.
### Configuration
Run `git_sync config` to create an initial configuration in `$XDG_CONFIG_HOME`, or pass `--file path/to/file` to create it elsewhere. If you do set a custom configuration path, you will need to specify the path with `-f` or `--file` for every command you run. This file, by default, is called 'git-manage.toml'. The generated configuration file has comments in it to aid you in setting everything up.

The important things to add to this configuration file are as follows:

- Your Github api token
- Your repositories in their correct category (public, private, fork, or a custom group). Forks should be anything that has a parent repository
- Your slack webhook url, if you have enabled Slack integration
- Optionally, a list of licenses in their spdx id format that you wish to blacklist.

Example of a valid repo section in the configuration file:
```toml
[repo]
# These should be repositories that have a defined upstream project
fork = ["https://github.com/my-org/repo", "https://github.com/my-org/some-other-repo"]
# This can be any public repository you have read/write access to
public = []
# These are repositories that are private to you or your organization
private = ["https://github.com/my-org/my-secret-repo"]
# Now an arbitrary group called "my-group" that contains two repositories
my-group = ["https://github.com/my-org/repo1", "https://github.com/my-org/repo2"]
some-other-group = []
group3 = []
```

Using these groupings, you can create groups of similar repositories that you can manage together. This can be for a specific repository or several repositories that have a similar structure.
If you use `--repository-type all` to run some command against every single repository configured in your configuration file, repositories will only be processed once, even if they are listed in multiple groups.

Certain commands require that you have git installed and available in your PATH.
That includes the following:
1. `git_sync backup`
2. `git_sync tag sync --with-annotated`

You can set the number of parallel jobs to run at a time by specifying the --jobs flag. By default, this is set to the total number of CPU threads the machine has, but it can be manually set to any positive integer between 1 and 64. If this is unspecified, and it can't be determined automatically, it will default to 4.

If you have a forked repository on Github that does not have a configured parent repository, you can put it into the fork_with_workaround map, where "forked repo" = "actual upstream repo". Ex: `fork_with_workaround = {"https://github.com/my-org/livy" = "https://github.com/apache/incubator-livy"}`. This will require that you have write access to your forked repository, and git set up correctly on your machine. 

### Tests

There are a few tests for some of the helper functions, and they can be executed by running `cargo test` in the source tree.

### Making changes

Before trying to commit any changes, ensure that you run `cargo fmt` to make sure that there are no formatting inconsistencies. There is API reference [hosted on Github](https://jeffreysmith.github.io/git_sync/git_sync/) should you need to lookup information about any of the functions, types, macros, or enums.

## Usage examples

By default, git_sync uses the `fork` group from the git-manage.toml file for the `--repository-type` flag. You can also specify `private`, `public`, `all`, or `custom`. Using a custom group requires specifying the name of the repository group you added to `git-manage.toml`. These repositories are only used when you specify `--all` as a target for some command; if you specify a specific repository with --repository, that repository will be used regardless of any group it may be in.

When using a `custom` group, you must also specify the `--group <group_name>` flag to indicate which custom group you want to use.

Be careful with `--repository-type all` as it will apply to every single repository within your configuration file, including custom groups. This is useful for querying information about all of your repositories, but can cause issues if you try to create a release for 

### Syncing a fork with its parent

If you want to sync all branches for a given repository, you can use the `--recursive` option. Note that this will take a lot longer because it needs to fetch all branches from the upstream parent as well as the fork branch to get the branches in both; any branches that are present in both where the parent has newer commits will be synced. When used with `--all`, this also requires the `--force` flag to ensure that you really mean to do that.

If you do not specify the `--recursive` option, only the primary branch of the upstream repository will be synced. This is usually 'main' or 'master'.

The `--recursive` option is important when tags in your parent repositories don't only point to the main branch. In that case, you'll either want to use `--recursive` to sync all out of date branches, or use `--branch` to target a specific branch to sync. This option isn't actually recursively going over anything, but I wanted to keep the nomenclature similar to the `rm` command since that's what many people will be familiar with.

Make sure you only run this for repositories that are actually forks of another repository and that Github detects as having an upstream project. It will not work otherwise.

```bash
$ git_sync repo sync --repository https://github.com/my-org/my-forked-repo # Sync a specific repository
$ git_sync repo sync --all --slack # Sync all configured repositories and send a slack notification
$ git_sync --repository-type custom --group my-group repo sync --all --slack # Sync all repositories in the "my-group" custom group

$ git_sync repo sync --all --recursive --force --slack # Go through all branches and sync those that aren't up to date.
$ git_sync repo sync -r https://github.com/my-org/my-forked-repo --branch my_branch_to_update 
```

If you want to sync a repository that does not have a parent configured in Github, you must place it in the `fork_with_workaround` group in the configuration file. Once you have done that, you can add the `--with-fork-workaround` flag to sync all of the common branches between the two repositories. NOTE: this only support merges that can be fast forwarded. If it cannot, you will need to sync that branch manually.

### Syncing tags
Before syncing tags, you should sync your fork with its parent repository to ensure that all references that a new tag may point to exist in your fork. At the very least, sync with the repository's main branch, which is the default for `repo sync`. Some repositories' tags point to a specific commit branch, and if you add a tag that does that without having the commits already, the tag may not sync correctly.


The `--with-annotated` flag will ensure that annotated tags are synced. To do this, git must be installed and available in your PATH, and you must have read and write access to the repository you are syncing. This is because you cannot sync annotated tags correctly with the Github API alone. Since it requires additional setup, it is not enabled by default. When it does clone the repository, it will pull in the least amount of references possible to minimize the amount of data being transferred, which helps speed the process up. No cleanup is required since it uses a temporary directory that is automatically deleted at the end of the process. This also alleviates any permissions issues that could occur since the git repository will be stored in a folder the user will be able to read.

```bash
$ git_sync tag sync -r https://github.com/my-org/my-forked-repo --slack
$ git_sync tag sync --all --with-annotated -j4 # With a maximum of 4 parallel jobs
```

**TODO** Add `--fork_with_workaround` and its functionality to syncing tags.

**Note**: 
  Syncing tags can still fail to work the way you expect if an upstream tag exists that points to a commit that is not present in your fork. This situation could occur if your repository is missing some commits or if it's missing branches that the tags point to.
### Backing up a repository

Creating a backup of a repository is one of the slowest operations that this tool can do, particularly for larger repositories. This is because it has to do a `git clone --mirror` for each repository, in order to preserve all files and metadata. 

When running the backup with `--all`, if you run it from an interactive terminal, you will be presented with a progress bar to show you how many of your repositories have been backed up so far. When this is used to backup many repositories (the largest number tested so far has been 40 at once), it can easily take 10 to 20+ minutes.


#### Local backup
Ensure you have enough space on your local filesystem to house all of your backups since these can be surprisingly large. For example, a `git clone --mirror` of [ClickHouse](https://github.com/ClickHouse/ClickHouse) is around 1.8GB on its own. You must have read-write access to this folder.


Importantly, the path you pick here must be a folder. 
```bash
$ git_sync backup create -r https://github.com/my-org/my-repo --path /path/to/backup/folder 
$ git_sync backup create --all -p /path/to/backup/folder --slack
# Backing up every single repsitory listed in the configuration file
$ git_sync backup create --repository-type all -p /path/to/backup/folder --slack
```

When using --repository-type all, any repositories listed in more than one place or group will only be backed up once.

#### S3 backup
Back ups can also be automatically uploaded to S3. This requires that you have the `aws` feature enabled at build time, and that you have configured your AWS credentials correctly. The bucket you are uploading to must already exist, and you must have write access to it. You may also need to set the name of your AWS profile in the terminal before starting the process.

You will still need to have enough hard drive space to store your backups. They will be compressed before uploading directly into your specified bucket. Uploading everything to AWS will make the entire backup process take longer.

To do so, you need to use both the `--destination s3` and `--bucket <bucket_name>` flags when uploading to S3. If you only set one of the two, you will get an error before the process starts.

```bash
$ aws sso login --profile my_aws_profile_name
$ export AWS_PROFILE=my_aws_profile_name

$ git_sync backup create -r https://github.com/my-org/my-repo --path /path/to/backup/folder --destination s3 --bucket my-bucket-name
$ git_sync backup create --repository-type all -p /path/to/backup/folder --destination s3 --bucket my-bucket-name --slack --all
```

This is an example of when using `--repository-type all` along with `--all` target is useful behaviour.

**TODO** Update documentation for using an IAM persistent user.

### Managing branches
You can create and delete branches for a single repository or for all configured repositories. This is useful when you have a set of common branches across all of your repositories.

```bash
$ git_sync branch create --all --new-branch my_new_branch --base-branch master
$ git_sync branch delete --all --branch my_branch_name
```

At the moment, running `branch delete` will immediately delete the branch without any confirmation. A future feature planned is a configurable delete queue that will create a 'cooling off' period before actually deleting the branch in order to avoid deleting branches permanently by accident. Support for this will come along with any SQL features since that will allow for persistent storage.

### Managing tags
Managing tags is very similar to managing branches. You can create and delete tags for a single repository or for all configured repositories. 

```shell
$ git_sync tag create --tag my_tag_name --branch the_branch_the_tag_points_to -r https://github.com/my-org/my-repo
$ git_sync tag delete -t my_tag_name --all
```

Just like with branches, running `delete` will immediately delete the tag. Support for a delete queue for a 'cooling off' period will come in the future.

If you just want to see the difference between a repository and its parent, you can also run `git_sync tag compare -r https://github.com/my-org/my-repo`. This will not make any modifications to any repositories.

In future, a 'cooling off' period will be added for tag deletions.

### Creating releases
Using this, you can create releases for a specific repository or all of them at one time. This requires knowing the previous release's name and the current release's name. Release notes will be automatically generated based on the difference between these two commits. Using the `--all` flag requires that all configured repositories have the same release tags present. Make sure that a release for this tag does not already exist, otherwise the command will fail.

```shell
$ git_sync release create --current-release v1.0.1 --previous-release v0.99.6 --all
```

You can also optionally specify the release name by setting the `--release-name <RELEASE NAME>` flag. If you don't specify it, it will use the name of the `--current-release` tag.
### Creating and merging pull requests
You can create and merge pull requests using the `pr open` command. There are many optional flags that can be used to customize the pull request, and if you would like to see all of them, run `git_sync pr open --help`. 

Not all branches can be merged automatically. If there are any merge conflicts, you will need to resolve them manually. The pull request will still be created even if it can't be merged automatically.

You can specify the most recent commit in your feature branch with the `--sha` flag. If you don't specify this, the SHA for the HEAD commit will be determined automatically since this information is required by the Github API.

Automatic merging requires specifying the `--merge` option. If you leave it out, there will be no attempt to merge the pull request automatically. Optionally, you can specify `--delete` when `--merge` is specified to automatically delete the branch after a successful merge. If the merge fails, the branch will not be deleted.

```shell
$ git_sync pr open -r https://github.com/my-org/my-repo --base main --head my_feature_branch --merge
$ git_sync pr open --all --base MY_MAIN_BRANCH --head my_feature_branch --merge
```

### Check repositories
This tool supports a few sanity checks for repositories. This includes checking the main license of the repository, checking for the status of branch protection rules, and checking for the presence of stale branches.

When running `git_sync repo check`, you must specify at least of of `--license`, `--protected`, or `--old-branches`. Specifying `--protected` requires that you also specify `--branch` since this is a branch specific check.

`--old-branches` has two optional flags that can be used to customize its behaviour:
1. `--days-ago`, which allows you to configure how old a branch must be before it is considered stale. The default is 30 days.
2. `--branch-filter` is a regex that you can use to filter which branches will be displayed. 

At the moment, little attention has been paid to checking for protection rules. This will be improved in the future.

The `blacklist` option in the `git-manage.toml` file is useful here. Any branch that you have specified in that list will be ignored when reporting stale branches. This is useful for long living branches that are intentionally not deleted.

```shell
$ git_sync repo check --repository https://github.com/my-org/my-repo --license --old-branches
$ git_sync repo check --all --license --protected --branch my_branch --old-branches --days-ago 90 --branch-filter ^HADOOP
```

If you want to create a csv file for visually checking all of your branches stale repositories, you can run the above like this:
```shell
$ git_sync repo check --repository-type all --all --license --old-branches --days-ago 90 > output.csv 
```

It will detect you are not running in an interactive terminal and will create a header, then output information in a comma separated format.
## Additional notes
- You will need a Github Token with both the 'repo' scope and the 'workflow' scope enabled, if using a classic token. Without these, syncing repositories may not work correctly.

- In **almost** every case, if you are trying to sync tags with an upstream repository, you will need to sync your fork before syncing the tags. If you don't, the references that the new tags point to may not yet exist in your fork which will cause your tag syncing to fail.

- There is a limit to the number of API requests you can make to Github in an hour. This limit is generally 5000 requests for the REST API, and around 5000 tokens for the GraphQL API. If you pass `--verbose` to your command, you will get the number of remaining requests you can make and when that limit will reset. Certain commands use REST API calls, while others use the GraphQL API which helps limit the impact of this limit.

## Getting help
All commands and subcommand have a `--help` flag that will give you information about the various flags, including valid options for each flag.

Rust documentation can be generated locally by running `cargo doc --no-deps --open`. You can also view the documentation online [hosted on Github](https://jeffreysmith.github.io/git_sync/git_sync/).

Deploying updates to the Github hosted documentation can be done by, when in the main branch, running `./generate_docs.sh` and then committing those new/changed files. Github will automatically deploy the updated documentation when this commit is pushed.

## Compatibility
This has been verified to run on both Redhat 7 and MacOS 15 meaning that it likely works on just about any Unix Operating System. It will likely not work on Windows since this tool expects a Unix environment, and has not been tested on Windows. No issues involving Windows will be addressed at this time.

## Generating man pages and shell completion
To generate new versions of the man pages, you can run `git_sync generate --kind man`.

To generate shell completion for bash, fish, or zsh, you can run `git_sync generate --kind [shell_type]` and then copy the output file them into your shell's completions directory.

For zsh, this is usually `/usr/local/share/zsh/site-functions/`
For bash, this is usually `/usr/share/bash-completion/completions/`
For fish, this is usually `~/.config/fish/completions` or `/etc/fish/completions`

The `generate` command will not show up during general usage, but it can always be run by specifying it directly.

Additional flags can be seen by running `git_sync generate --help`
