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
tiffany orchestrator
```

The tap repository should publish the formula at `Formula/tiffany-loop.rb`.
The formula installs both binaries:

- `orchestrator` for config, roles, event streaming, ACP, and non-interactive runs.
- `tiffany` for the primary TUI. Use `tiffany orchestrator`, or `orchestrator tui`
  which delegates to `tiffany` when it is installed.

After install, initialize runtime config and configure providers/roles from the TUI:

```bash
orchestrator init
tiffany orchestrator
```

Inside the TUI, use `/provider` and `/role`.

Note: if `macguffinQ/Tiffany` is private, public Homebrew installs cannot download the release archive or source archive. Maintainers with repository access can still use the formula for validation.

## Optional automatic tap updates

The release workflow includes a `homebrew` job. It runs only when the repository has a `HOMEBREW_TAP_TOKEN` secret.

Create a fine-grained GitHub token with contents read/write access to `macguffinQ/homebrew-tap`, then add it to this repository:

```text
Settings -> Secrets and variables -> Actions -> New repository secret
Name: HOMEBREW_TAP_TOKEN
```

After that, pushing a tag like `v0.1.6` will:

1. Build the macOS Apple Silicon release archive used by Homebrew.
2. Publish the GitHub Release.
3. Update `macguffinQ/homebrew-tap` with a `tiffany-loop` formula that installs both
   `orchestrator` and `tiffany`.

The generated formula uses the prebuilt macOS Apple Silicon archive on Apple Silicon Macs. Intel Macs, Linuxbrew, and other platforms build from the tagged source archive until additional release archives are wired into the formula.

The workflow computes checksums from authenticated GitHub APIs, so it works while the main repository is private and continues to work after the repository is public.

## Formula template

See `packaging/homebrew/tiffany-loop.rb`. It is a source-build template; the release workflow generates the published formula with real archive URLs and checksums.
