#!/usr/bin/env bash
# Replay one merge batch the old way or with merge-check, under a memory limit, and report the
# wall time of each step and the peak memory of everything it ran (docs/merge-check.md).
#
#   tools/merge-replay.sh old <base> <head> <worktree> <target-dir> <log-prefix> -p crate-a -p crate-b ...
#   tools/merge-replay.sh new <base> <head> <worktree> <target-dir> <log-prefix>
#
# old: `cargo clippy --workspace --all-targets -D warnings`, then one `cargo test --no-fail-fast`
#      with the given -p flags, on a target dir whose deps/ and incremental/ were deleted (as the
#      merger did after every batch).
# new: merge-check --stage merge, then --stage after, on a warm target dir.
# Each run gets a fresh detached worktree of <head>. Run it from a clone that has both revisions.
#
# Environment: MEM (memory limit of the run, default 20G), JOBS (CARGO_BUILD_JOBS, default 6),
# MERGE_CHECK (an xtask binary built with --no-default-features; required for `new`).
# Needs systemd-run --user (the run is a transient scope; its memory.peak is the peak).
set -u
mode=${1:?old|new} base=${2:?base} head=${3:?head} wt=${4:?worktree} tgt=${5:?target dir} log=${6:?log prefix}
shift 6
MEM=${MEM:-20G}
JOBS=${JOBS:-6}

git worktree remove --force "$wt" 2>/dev/null
rm -rf "$wt"
git worktree add -q --detach "$wt" "$head" || exit 1
mkdir -p "$tgt"
if [ "$mode" = old ]; then
  rm -rf "$tgt/debug/deps" "$tgt/debug/incremental"
fi

script=$(mktemp)
{
  echo '#!/usr/bin/env bash'
  echo '[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"'
  printf 'cd %q || exit 1\n' "$wt"
  printf 'export CARGO_TARGET_DIR=%q CARGO_BUILD_JOBS=%q CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0\n' "$tgt" "$JOBS"
  printf 'log=%q\n' "$log"
  echo 'cg=/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)'
  echo 't0=$(date +%s)'
  if [ "$mode" = old ]; then
    echo 'nice -n 15 cargo clippy --workspace --all-targets -- -D warnings > "$log-clippy.log" 2>&1; c=$?'
    echo 't1=$(date +%s)'
    printf 'nice -n 15 cargo test --no-fail-fast'
    printf ' %q' "$@"
    echo ' > "$log-test.log" 2>&1; t=$?'
    echo 't2=$(date +%s)'
    echo 'echo "old clippy=$((t1-t0))s rc=$c tests=$((t2-t1))s rc=$t total=$((t2-t0))s peak=$(cat $cg/memory.peak)" | tee "$log-result.txt"'
  else
    printf 'mc=%q base=%q head=%q\n' "${MERGE_CHECK:?MERGE_CHECK=path to the xtask binary}" "$base" "$head"
    echo 'nice -n 15 "$mc" merge-check "$base" "$head" --root . --stage merge > "$log-merge.log" 2>&1; m=$?'
    echo 't1=$(date +%s)'
    echo 'nice -n 15 "$mc" merge-check "$base" "$head" --root . --stage after > "$log-after.log" 2>&1; a=$?'
    echo 't2=$(date +%s)'
    echo 'echo "new merge=$((t1-t0))s rc=$m after=$((t2-t1))s rc=$a total=$((t2-t0))s peak=$(cat $cg/memory.peak)" | tee "$log-result.txt"'
  fi
} > "$script"
systemd-run --user --scope -q -p MemoryMax="$MEM" --unit="mreplay-$$" bash "$script"
rc=$?
rm -f "$script"
git worktree remove --force "$wt" 2>/dev/null
exit $rc
