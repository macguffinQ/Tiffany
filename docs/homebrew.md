# Homebrew Release Setup

This project publishes release archives from GitHub Actions. Homebrew support is handled by a separate public tap repository:

```text
macguffinQ/homebrew-tap
```

## Install

Homebrew is the recommended install path for users:

```bash
brew tap macguffinQ/tap
brew install tiffany-loop
tiffany-loop setup
tiffany-loop
```

The tap repository should publish the formula at `Formula/tiffany-loop.rb`.
The formula installs one multi-call binary exposed under three command names:

- `orchestrator` for config, roles, event streaming, ACP, and non-interactive runs.
- `tiffany-loop` for the primary TUI. Use this command first.
- `tiffany` as a compatibility alias for older scripts.

After install, initialize runtime config and configure providers/roles from the
TUI-facing command:

```bash
tiffany-loop setup
tiffany-loop
```

Inside the TUI, use `/provider` and `/role`.

Note: if `macguffinQ/Tiffany` is private, public Homebrew installs cannot download the release archive or source archive. Maintainers with repository access can still use the formula for validation.

## Upgrade Or Stale Tap Recovery

Use `brew upgrade` or `brew reinstall` for an existing install. `brew install`
can legitimately print "already installed and up-to-date" when the local tap
checkout is stale, even if GitHub already has a newer formula.

```bash
brew update
brew upgrade macguffinQ/tap/tiffany-loop
tiffany-loop --version
```

If the version is still old, reset the local tap checkout:

```bash
brew untap macguffinQ/tap
brew tap macguffinQ/tap
brew reinstall macguffinQ/tap/tiffany-loop
tiffany-loop --version
orchestrator --version
```

To inspect what Homebrew is actually using:

```bash
brew --repo macguffinQ/tap
sed -n '1,16p' "$(brew --repo macguffinQ/tap)/Formula/tiffany-loop.rb"
```

If Homebrew reports that `tiffany-loop` is installed but the shell cannot find
the command, first check the package prefix and PATH:

```bash
brew --prefix tiffany-loop
brew list --versions tiffany-loop
eval "$(brew shellenv)"
tiffany-loop doctor
```

`tiffany-loop status` prints the resolved `tiffany-loop` command, the
compatibility `tiffany` alias, the `orchestrator` bridge command, and the active
config roots. `orchestrator doctor` checks the Homebrew tap, package version,
installed binary paths, and whether `tiffany-loop`, `tiffany`, and
`orchestrator` resolve through `PATH`. If the prefix is present but commands are
missing, use `brew reinstall tiffany-loop`; if the binaries exist but are not
found, add Homebrew's `bin` directory to the shell startup file.

## Automatic tap updates

The release workflow includes a `homebrew` job. When `HOMEBREW_TAP_TOKEN` is
configured, tagged releases automatically update the Homebrew tap after the
GitHub Release is published. If the secret is missing, the GitHub Release still
publishes successfully, but the `homebrew` job fails and the workflow summary
prints the manual tap update path. Treat that red job as intentional: Homebrew
is not synchronized until the tap commit is pushed and post-release checks pass.

Create a fine-grained GitHub token with contents read/write access to `macguffinQ/homebrew-tap`, then add it to this repository:

```text
Settings -> Secrets and variables -> Actions -> New repository secret
Name: HOMEBREW_TAP_TOKEN
```

After that, pushing a tag like `vX.Y.Z` will:

1. Run `./scripts/tiffany-release-preflight --full --tag <tag>` on the tagged commit.
2. Build the macOS Apple Silicon release archive used by Homebrew.
3. Publish the GitHub Release.
4. Update `macguffinQ/homebrew-tap` with a `tiffany-loop` formula that exposes
   the same binary as `tiffany-loop`, `orchestrator`, and the compatibility alias
   `tiffany`.

The current release matrix publishes a prebuilt `aarch64-apple-darwin` archive.
The generated formula uses that archive on Apple Silicon Macs. Intel Macs,
Linuxbrew, and other platforms build from the tagged source archive until
additional release archives are wired into the workflow and formula.

The workflow calls `./scripts/tiffany-update-homebrew-tap` to generate and
commit the formula. Keep formula changes in that script so automatic releases
and manual tap repairs stay identical. The generated formula uses the public
release asset URL and the public tagged source archive URL, and computes
checksums from those exact URLs so Homebrew validates the same bytes it
downloads.

Tagged preflight checks enforce a 24-hour cooldown after the previous release
tag. This keeps small fixes batched under `CHANGELOG.md` `Unreleased` instead
of producing noisy patch releases. For an urgent installer, startup, or security
fix, set the repository variable `TIFFANY_RELEASE_ALLOW_FREQUENT=1` for that
tag push, then remove it after the release.

Tagged preflight also fails when the requested tag already exists but does not
point at the checked-out commit. Prefer cutting a new patch tag. Only set
`TIFFANY_RELEASE_ALLOW_TAG_REWRITE=1` for the local preflight after explicitly
deciding to force-move an existing tag.

If the automatic tap update fails, confirm the GitHub Release, tap commit, and
formula with:

```bash
gh run list --repo macguffinQ/Tiffany --workflow Release --limit 5
gh run view <run-id> --repo macguffinQ/Tiffany --json jobs
git ls-remote --heads https://github.com/macguffinQ/homebrew-tap.git main
```

Manual tap update fallback from this repository:

```bash
tag=vX.Y.Z
./scripts/tiffany-update-homebrew-tap --tag "$tag" --tap-dir ../homebrew-tap

cd ../homebrew-tap
ruby -c Formula/tiffany-loop.rb
git add Formula/tiffany-loop.rb
git commit -m "Update tiffany-loop to ${tag}"
git push
```

Use `--dry-run` to print the generated formula without writing files, or
`--commit --push` when the tap remote and credentials are already correct.

After pushing the tap, verify the published archive before telling users to install:

```bash
./scripts/tiffany-post-release-check --tag "$tag"
```

Use `--skip-install` to verify only the GitHub Release asset and Homebrew tap
formula checksums without running `brew upgrade`, `tiffany-loop doctor`, and
`brew test`.

From a source checkout with matching binaries built, maintainers can also run
the isolated install smoke against a binary directory:

```bash
./scripts/tiffany-install-smoke --bin-dir /path/to/tiffany-loop-vX.Y.Z-aarch64-apple-darwin
```

## Formula template

See `packaging/homebrew/tiffany-loop.rb`. It is a source-build template; the release workflow generates the published formula with the current tag, archive URLs, and checksums.

The release preflight checks the template syntax when Ruby is available:

```bash
ruby -c packaging/homebrew/tiffany-loop.rb
./scripts/tiffany-release-preflight --quick
```
