# merge-check: verifying a merge batch

`cargo merge-check <base> <head>` (or `cargo xtask merge-check`, which builds the engine first)
verifies a batch of merged branches with the same coverage as "clippy the whole workspace, then
`cargo test` every crate the batch touched", in a fraction of the time. Source:
`xtask/src/mergecheck.rs`.

```sh
# in the merge worktree, with the batch committed on top of <base>:
nice -n 15 cargo merge-check <base> HEAD                # clippy + tests + doc-tests (the merge stage)
nice -n 15 cargo merge-check <base> HEAD --stage after  # the slow tests, once the batch is merged
cargo merge-check <base> HEAD --dry-run                 # what changed, which packages, which commands
```

## What it checks

1. **Changed files** (`git diff --name-only base head`) map to workspace packages: a file in a
   package's directory is that package; `Cargo.toml`, `Cargo.lock`, `clippy.toml`,
   `rust-toolchain*`, `.cargo/` and `.config/` are every package; any other file, and a package
   file another package reads by path, maps to every package whose sources name it as
   `../<dir>` or `join("<dir>")` (a test reading `docs/compare-facts.json`, a build script reading
   `assets/`). Files nothing reads (plans, notes) are listed and need no build.
2. **Affected packages**: those packages, every package that depends on them (normal and build
   dependencies, transitively), and every package that uses one of them as a dev-dependency.
   "An engine change also tests ui-egui and the CLI" falls out of this.
3. **clippy** once, for the affected packages, `--all-targets -D warnings` (`--workspace` when every
   package is affected). A package outside the set has the same sources and the same
   dependencies as on the base, which passed, so its result cannot change.
4. **Tests** with [cargo-nextest](https://nexte.st) (installed with `cargo install cargo-nextest
   --locked` when missing; `--no-install` or a failed install falls back to plain `cargo test`):
   all test binaries run side by side (`--test-threads`, default `$CARGO_BUILD_JOBS`, else 6)
   instead of one binary after another. Doc-tests run with `cargo test --doc`.
   - **Slow tests.** Every run records each test's time in `<target>/merge-check/timings.json`
     (the last 3 runs). A test whose slowest recorded run reaches `--slow-secs` (default 60) runs in
     the **after** stage instead (`--stage after`, `--after-threads`, default 2). The two stages'
     filters are exact complements, so together they run every test the single `cargo test` ran.
     A test with no record yet (new) runs in the merge stage.
   - **Failures under load.** Timing tests and kill-a-child tests can fail while a dozen test
     processes and other builds share the machine. When some tests fail (at most 50, and the
     build was fine), merge-check re-runs just those, alone, one at a time: a failure that repeats
     fails the check; one that passes alone is listed ("passed alone") and the pass counts as
     green, as a merger re-running a flaky test by hand would decide.
   - **Serialized test files.** An integration test file that serializes its tests with a
     `static ...: Mutex<()>` runs one test at a time (a nextest test group per binary), as it did
     inside one `cargo test` process.
5. **Failure output** is the tools' own: clippy's diagnostics; nextest prints each failing test's
   output and panic as it fails and again at the end. merge-check then lists the failing tests
   with the plain `cargo test ... -- --exact <name>` command that re-runs each one, and exits
   non-zero.

## The build cache

One target directory (`--target-dir`, else `$CARGO_TARGET_DIR`, else `<root>/target`) is kept warm
between batches: third-party crates and untouched workspace crates are not rebuilt.

- **Fresh worktrees.** A new checkout gives every file a new mtime, which makes cargo rebuild the
  whole workspace. merge-check records each tracked file's SHA-256 and mtime after a run; next
  time, a file with the same content gets its recorded mtime back, unless a cargo build outside
  merge-check touched that file's package since (then cargo's own rule applies to it: new mtime,
  rebuild). Changed files keep their new mtime and rebuild as usual. `--no-mtime-restore` turns
  this off. Keeping one persistent merge worktree (updated with `git checkout`/`reset`, which only
  rewrites changed files) has the same effect without it.
- **Size cap** (`--cap-gb`, default 120): after the run, if the directory is larger, the
  incremental caches are removed; if it is still larger, the build outputs are removed (the next
  run builds cold). Timing data survives.
- Debug info is off unless set (`CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`); keep
  the same settings between runs, or the cache holds two builds of everything.
- sccache is not needed with a warm directory; it would only help a cold one.

## Merge server setup

- One persistent cache outside the worktree, e.g. `CARGO_TARGET_DIR=~/wt/merge-target`, never
  deleted after a batch (the cap keeps it bounded; cargo overwrites a crate's outputs in place when
  its sources change, so the size grows only when dependency versions or the toolchain change).
- `CARGO_BUILD_JOBS` as the server's memory allows; `nice -n 15`.
- cargo-nextest installs itself on first use (a few minutes, once).
- The timing data lives in the cache directory (`merge-check/timings.json`); the first run has
  none, so it runs every test in the merge stage and records the times.
- `tools/merge-replay.sh` replays a batch the old way and the new way under one memory limit and
  reports wall time and peak memory (how the numbers in the revision history were measured).

## Revision history

- 0.7.10: first version (affected-package closure, nextest with an after stage for slow tests,
  warm cache with mtime restore and size cap).
