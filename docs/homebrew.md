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
tiffany-loop
```

The tap repository should publish the formula at `Formula/tiffany-loop.rb`.
The formula installs three binaries:

- `orchestrator` for config, roles, event streaming, ACP, and non-interactive runs.
- `tiffany-loop` for the primary TUI. Use this command first.
- `tiffany` as a compatibility alias for older scripts.

After install, initialize runtime config and configure providers/roles from the TUI:

```bash
orchestrator init
tiffany-loop
```

Inside the TUI, use `/provider` and `/role`.

Note: if `macguffinQ/Tiffany` is private, public Homebrew installs cannot download the release archive or source archive. Maintainers with repository access can still use the formula for validation.

## Required automatic tap updates

The release workflow includes a `homebrew` job. Tagged releases require a
`HOMEBREW_TAP_TOKEN` secret so the GitHub Release and Homebrew tap cannot drift.
If the secret is missing, the release workflow fails after publishing assets
instead of silently leaving the tap on an older formula.

Create a fine-grained GitHub token with contents read/write access to `macguffinQ/homebrew-tap`, then add it to this repository:

```text
Settings -> Secrets and variables -> Actions -> New repository secret
Name: HOMEBREW_TAP_TOKEN
```

After that, pushing a tag like `v0.1.12` will:

1. Run `./scripts/tiffany-release-preflight --quick --tag <tag>` on the tagged commit.
2. Build the macOS Apple Silicon release archive used by Homebrew.
3. Publish the GitHub Release.
4. Update `macguffinQ/homebrew-tap` with a `tiffany-loop` formula that installs
   `tiffany-loop`, `orchestrator`, and the compatibility alias `tiffany`.

The current release matrix publishes a prebuilt `aarch64-apple-darwin` archive.
The generated formula uses that archive on Apple Silicon Macs. Intel Macs,
Linuxbrew, and other platforms build from the tagged source archive until
additional release archives are wired into the workflow and formula.

The workflow computes checksums from authenticated GitHub APIs, so it works while the main repository is private and continues to work after the repository is public.

If the tap job fails, confirm the GitHub Release, tap commit, and formula with:

```bash
gh run list --repo macguffinQ/Tiffany --workflow Release --limit 5
gh run view <run-id> --repo macguffinQ/Tiffany --json jobs
git ls-remote --heads https://github.com/macguffinQ/homebrew-tap.git main
```

Manual tap update fallback:

```bash
tag=v0.1.12
version="${tag#v}"
asset="tiffany-loop-${tag}-aarch64-apple-darwin.tar.gz"

arm_sha="$(
  gh release view "$tag" \
    --repo macguffinQ/Tiffany \
    --json assets \
    --jq ".assets[] | select(.name == \"${asset}\") | .digest" |
    sed 's/^sha256://'
)"
source_sha="$(
  gh api "repos/macguffinQ/Tiffany/tarball/${tag}" |
    shasum -a 256 |
    awk '{print $1}'
)"

git clone https://github.com/macguffinQ/homebrew-tap.git /tmp/homebrew-tap
cd /tmp/homebrew-tap
# Edit Formula/tiffany-loop.rb to use $version, $tag, $arm_sha, and $source_sha.
ruby -c Formula/tiffany-loop.rb
git add Formula/tiffany-loop.rb
git commit -m "Update tiffany-loop to ${tag}"
git push
```

After pushing the tap, verify the published archive before telling users to install:

```bash
gh release download "$tag" \
  --repo macguffinQ/Tiffany \
  --pattern "$asset" \
  --dir /tmp \
  --clobber
shasum -a 256 "/tmp/${asset}"
tar -tzf "/tmp/${asset}" | head
```

From a source checkout with matching binaries built, maintainers can also run
the isolated install smoke against a binary directory:

```bash
./scripts/tiffany-install-smoke --bin-dir /path/to/tiffany-loop-v0.1.12-aarch64-apple-darwin
```

## Formula template

See `packaging/homebrew/tiffany-loop.rb`. It is a source-build template; the release workflow generates the published formula with the current tag, archive URLs, and checksums.

The release preflight checks the template syntax when Ruby is available:

```bash
ruby -c packaging/homebrew/tiffany-loop.rb
./scripts/tiffany-release-preflight --quick
```
