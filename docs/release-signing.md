# Release signing

The desktop updater installs a release only when its `SHA256SUMS` carries a valid ed25519
signature (`SHA256SUMS.sig`) from a key listed in `apps/cadaddy/src/release_keys.txt`, over the
release's version and the sums; the installer must match the signed SHA-256 (and GitHub's own
digest, when it lists one) and be newer than the running version. Format: `apps/cadaddy/src/relsig.rs`.

## Every release
1. Put the release files (installers, `.mcpb`, …) in one folder.
2. `CADADDY_RELEASE_KEY=/path/to/release.key cargo xtask sign-release FOLDER` (version from
   `Cargo.toml`, or `--version X.Y.Z`): writes `SHA256SUMS` and `SHA256SUMS.sig` and checks them.
3. Upload `SHA256SUMS` and `SHA256SUMS.sig` with the installers. A release without them can't be
   installed by the updater (users can still download it from the release page).

`cargo xtask verify-release FOLDER` repeats the updater's check.

## Keys
- `cargo xtask release-key --out FILE` makes a key: the secret goes to FILE (owner-only), never into
  this repository; keep it backed up offline. Losing every listed key means builds out there can't
  update themselves any more.
- To rotate: add the new public key to `release_keys.txt`, ship a release signed with the old key,
  then sign with the new one; remove the old key only after that.
