Guide: all the distribution channels

Status: crates.io, Homebrew, and the shell/PowerShell installers are all LIVE
as of v0.4.2 (July 2026). Windows MSI/winget/Scoop and Linux packaging below
remain future options.

1. crates.io (cargo install) — LIVE

Published as claude-profiles (installs the claude-profile binary). To ship a
new version after bumping Cargo.toml and tagging the release:

cargo publish        # from the repo root; needs `cargo login` once with a token
                     # from https://crates.io/settings/tokens

Note: crates.io publishes are permanent — you can yank a bad version but never
delete it. Users install/update with `cargo install claude-profiles`.

2. Homebrew — LIVE

Publishes automatically on each tagged release via the publish-homebrew-formula
job in release.yml. Config that makes it work (already in Cargo.toml under
[workspace.metadata.dist]):

  installers   = [..., "homebrew"]
  tap          = "fuzzyalej/homebrew-tap"
  publish-jobs = ["homebrew"]

The job pushes Formula/claude-profiles.rb into the fuzzyalej/homebrew-tap repo
using the HOMEBREW_TAP_TOKEN repo secret (a fine-grained PAT with Contents:
read+write on the tap repo).

Users install/update with:
  brew install fuzzyalej/tap/claude-profiles
  brew upgrade claude-profiles

Reusing the tap for future tools: the same tap repo + token work for any other
tool — point that project's dist config at tap = "fuzzyalej/homebrew-tap" and
copy the HOMEBREW_TAP_TOKEN secret into its repo. Each project writes its own
Formula/<name>.rb; they don't collide.

Gotcha (learned the hard way): the tap repo must already have an initial commit
(a main branch) before the first publish, or the job fails with "couldn't find
remote ref refs/heads/main". Seed it with a README once.

3. Windows

- MSI installer — pure config, no external account needed. Add "msi" to your installers list, dist generate regenerates the workflow to build it, done. Gives users a double-clickable installer alongside the existing PowerShell script.
- winget (the real "easy button" for Windows users) — requires submitting a manifest to the community microsoft/winget-pkgs repo. Not something cargo-dist automates; typically done with wingetcreate pointed at your release's .zip/.msi + checksum, opened as a PR. First submission needs a human review from Microsoft's bot + maintainers.
- Scoop — lighter-weight than winget: a single JSON manifest in a bucket repo (can be your own, e.g. fuzzyalej/scoop-bucket, no review process). Good middle ground if winget feels like too much ceremony.

4. Linux, beyond the shell script

cargo-dist's installer kinds are actually shell, powershell, npm, homebrew, msi, pkg (I confirmed this directly against the installed dist binary) — note pkg is a macOS .pkg installer, not a Linux package format. For real Linux package-manager reach, it's external tooling:
- .deb — easiest lift: cargo-deb builds a .deb from your existing binary as one more CI step, attach it to the GitHub Release. No repo hosting needed for dpkg -i installs.
- AUR (Arch) — a PKGBUILD referencing your release tarball, published to aur.archlinux.org. Needs an AUR account + SSH key, and you maintain it going forward (bumping pkgver per release, ideally scripted).
- Nix — a flake.nix in-repo lets nix run github:fuzzyalej/claude-profile work with zero publishing step at all; submitting to nixpkgs proper is heavier and usually not worth it at this stage.

Suggested order for what's left

crates.io, Homebrew, and shell/PowerShell are already live. Remaining options by
effort vs. payoff: MSI (just a config flag) → .deb if you want a Linux artifact →
winget/AUR/Scoop only if you want maximum discoverability and are OK maintaining
them long-term.
