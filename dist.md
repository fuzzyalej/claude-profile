Guide: all the distribution channels

1. crates.io (cargo install)

Repo-side, everything's ready (renamed package, dry-run verified). What's left is yours to do, since it needs your account:

cargo login          # paste the API token from https://crates.io/settings/tokens
cargo publish

Users then run cargo install claude-profiles (installs the claude-profile binary). One thing to know: crates.io publishes are permanent — you can yank a bad version but never delete it, so this is worth doing deliberately, not as a rehearsal.

2. Homebrew

Your Cargo.toml already lists homebrew under installers, but I checked dist plan — there's no actual publish job configured, because cargo-dist needs a tap target to push a formula to. Right now installers = ["homebrew"] just means "the announcement text will mention brew install," not that it actually happens.

To make it real:
1. Create a new GitHub repo named homebrew-tap under fuzzyalej (Homebrew's naming convention for third-party taps).
2. Add to Cargo.toml:
tap = "fuzzyalej/homebrew-tap"
3. Create a GitHub PAT (classic, repo scope) that can push to that tap repo, add it as a repository secret here named HOMEBREW_TAP_TOKEN.
4. Run dist generate again — it'll wire up a real publish job in release.yml that pushes the formula on each release.

Then: brew tap fuzzyalej/tap && brew install claude-profile.

3. Windows

- MSI installer — pure config, no external account needed. Add "msi" to your installers list, dist generate regenerates the workflow to build it, done. Gives users a double-clickable installer alongside the existing PowerShell script.
- winget (the real "easy button" for Windows users) — requires submitting a manifest to the community microsoft/winget-pkgs repo. Not something cargo-dist automates; typically done with wingetcreate pointed at your release's .zip/.msi + checksum, opened as a PR. First submission needs a human review from Microsoft's bot + maintainers.
- Scoop — lighter-weight than winget: a single JSON manifest in a bucket repo (can be your own, e.g. fuzzyalej/scoop-bucket, no review process). Good middle ground if winget feels like too much ceremony.

4. Linux, beyond the shell script

cargo-dist's installer kinds are actually shell, powershell, npm, homebrew, msi, pkg (I confirmed this directly against the installed dist binary) — note pkg is a macOS .pkg installer, not a Linux package format. For real Linux package-manager reach, it's external tooling:
- .deb — easiest lift: cargo-deb builds a .deb from your existing binary as one more CI step, attach it to the GitHub Release. No repo hosting needed for dpkg -i installs.
- AUR (Arch) — a PKGBUILD referencing your release tarball, published to aur.archlinux.org. Needs an AUR account + SSH key, and you maintain it going forward (bumping pkgver per release, ideally scripted).
- Nix — a flake.nix in-repo lets nix run github:fuzzyalej/claude-profile work with zero publishing step at all; submitting to nixpkgs proper is heavier and usually not worth it at this stage.

Suggested order

Given effort vs. payoff: crates.io (you're one cargo publish away) → MSI (just a config flag) → Homebrew tap (needs a new repo + token, ~10 min) → .deb if you want a Linux artifact → winget/AUR only if you want maximum discoverability and are OK maintaining them long-term.

Want me to wire up the MSI installer now (that one's fully local, no accounts needed)?
