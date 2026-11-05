#!/usr/bin/env python3
"""Measure one build of a job for the accuracy trend (Q6193), with no agent (no usage cost).

    python3 -I scripts/accuracytrend.py --config trend-config.json [--commit SHA] [--if-new]
                                        [--cli PATH] [--model BUILT.cdy] [--no-copy]

1. Takes the job-run lock (one job run on the machine at a time; another holder = exit 3).
2. Resolves the commit (default: the hub branch's tip) and, with --if-new, stops when the trend's
   newest row already is that commit (exit 0): the loop hook.
3. Builds that commit's cadaddy-cli in a worktree (or takes --cli).
4. Runs the job on a fresh copy of the job's checkpoint (job.start with the config's `job`
   parameters, resume "any"), timing it and reading its peak memory from its progress lines, under
   the time limit (default 2 h: a run that reaches it is a finding, exit 4, never restarted).
5. Measures the built model (`cadaddy-cli accuracy`: validate.measure + validate.trend): one row
   keyed by the commit, appended to the trend file and compared with the build before, the
   commits between them (git) and the merger's batch log naming the suspects.
6. Copies the trend file, its table and plot to `copy_to` (scp).

Exit status: 0 measured (or nothing new), 2 measured and a metric regressed, 3 the lock is held,
4 the job reached the time limit, 1 anything else.

The config is the measuring machine's own file (it names the job's private paths; never commit
one). Keys: repo, hub (git remote, "origin"), branch ("next"), workdir, target_dir, cargo_jobs (4),
checkpoint, job {pdfs, submittals?, specs?, goal?, ...}, rooms [number | {room, level}], skip_rules,
label, trend, merge_log (a local path) or fetch_merge_log ("host:path", scp), copy_to
("host:dir/", scp), lock, time_limit_s (7200), measure_cli (a CLI with `accuracy`, for builds
older than it), last (rows in the table). See docs/accuracy-trend.md.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time

EXE = ".exe" if os.name == "nt" else ""


def log(msg):
    print(f"[accuracytrend {time.strftime('%Y-%m-%d %H:%M:%S')}] {msg}", flush=True)


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True, check=True).stdout.strip()


class Lock:
    """An exclusive lock file holding the holder's pid and start time."""

    def __init__(self, path):
        self.path = path
        self.held = False

    def __enter__(self):
        if not self.path:
            return self
        os.makedirs(os.path.dirname(os.path.abspath(self.path)), exist_ok=True)
        try:
            fd = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError:
            holder = open(self.path, encoding="utf-8", errors="replace").read().strip()
            log(f"another job run holds the lock {self.path}: {holder}")
            sys.exit(3)
        with os.fdopen(fd, "w") as f:
            f.write(f"accuracytrend pid {os.getpid()} since {time.strftime('%Y-%m-%dT%H:%M:%S')}\n")
        self.held = True
        return self

    def __exit__(self, *exc):
        if self.held:
            try:
                os.remove(self.path)
            except OSError:
                pass


def last_row(trend):
    try:
        lines = [l for l in open(trend, encoding="utf-8").read().splitlines() if l.strip()]
    except OSError:
        return None
    for l in reversed(lines):
        try:
            return json.loads(l)
        except ValueError:
            continue
    return None


def same_commit(a, b):
    a, b = (a or "").lower(), (b or "").lower()
    return len(a) >= 7 and len(b) >= 7 and (a.startswith(b) or b.startswith(a))


def build_cli(cfg, commit, work):
    """cadaddy-cli built at `commit` in a worktree of the repo; its path."""
    repo = cfg["repo"]
    tree = os.path.join(work, "src")
    if not os.path.isdir(os.path.join(tree, ".git")) and not os.path.isfile(os.path.join(tree, ".git")):
        git(repo, "worktree", "add", "--detach", tree, commit)
    else:
        git(tree, "checkout", "-q", "--detach", commit)
    env = dict(os.environ)
    env["CARGO_BUILD_JOBS"] = str(cfg.get("cargo_jobs", 4))
    if cfg.get("target_dir"):
        env["CARGO_TARGET_DIR"] = cfg["target_dir"]
    target = env.get("CARGO_TARGET_DIR", os.path.join(tree, "target"))
    log(f"building cadaddy-cli at {commit[:10]} (target {target})")
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "cadaddy-cli"], cwd=tree, env=env, check=True)
    exe = os.path.join(target, "release", "cadaddy-cli" + EXE)
    keep = os.path.join(work, f"cadaddy-cli-{commit[:10]}{EXE}")
    shutil.copy2(exe, keep)
    return keep


PEAK = re.compile(r"peak (?:memory )?(\d+) MB")


def run_job(cfg, cli, commit, work):
    """The job on a fresh copy of the checkpoint: (built model, wall seconds, peak MB). The job
    starts in a new drawing and goes on from the copy (a drawing holding the job open would be
    "a job already running")."""
    short = commit[:10]
    ckpt = os.path.join(work, f"ckpt-{short}.cdy")
    built = os.path.join(work, f"built-{short}.cdy")
    for p in (ckpt, built):
        if os.path.exists(p):
            os.remove(p)
    shutil.copy2(cfg["checkpoint"], ckpt)
    params = {"goal": "takeoff", "auto": True, "resume": "any"}
    params.update(cfg.get("job", {}))
    params["checkpoint"] = ckpt.replace("\\", "/")
    cmds = os.path.join(work, f"job-{short}.jsonl")
    with open(cmds, "w", encoding="utf-8", newline="\n") as f:
        f.write("job.start " + json.dumps(params) + "\n")
        f.write("job.status {}\n")
    out, err = os.path.join(work, f"job-{short}.out"), os.path.join(work, f"job-{short}.err")
    limit = float(cfg.get("time_limit_s", 7200))
    log(f"job started (limit {limit:.0f} s)")
    t0 = time.time()
    with open(out, "w", encoding="utf-8") as fo, open(err, "w", encoding="utf-8") as fe:
        p = subprocess.Popen([cli, "run", "--cmd-file", cmds, "--save", built], stdout=fo, stderr=fe)
        try:
            code = p.wait(timeout=limit)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()
            log(f"FINDING: the job at {short} reached the {limit:.0f} s limit; not restarted (see {err})")
            sys.exit(4)
    secs = time.time() - t0
    peak = 0
    for line in open(err, encoding="utf-8", errors="replace"):
        for m in PEAK.finditer(line):
            peak = max(peak, int(m.group(1)))
    if os.name != "nt":
        import resource

        kb = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        peak = max(peak, kb // 1024 if sys.platform != "darwin" else kb >> 20)
    if code != 0 or not os.path.exists(built):
        log(f"the job failed (exit {code}); see {err}")
        sys.exit(1)
    log(f"job done in {secs:.0f} s, peak {peak} MB")
    try:
        os.remove(ckpt)
    except OSError:
        pass
    return built, secs, peak


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--config", required=True)
    ap.add_argument("--commit")
    ap.add_argument("--if-new", action="store_true", help="stop when the trend's newest row is this commit")
    ap.add_argument("--cli", help="a cadaddy-cli for the build (skip building one)")
    ap.add_argument("--model", help="an already built model (skip the job)")
    ap.add_argument("--job-s", type=float)
    ap.add_argument("--peak-mb", type=float)
    ap.add_argument("--no-copy", action="store_true")
    a = ap.parse_args()
    cfg = json.load(open(a.config, encoding="utf-8"))
    # Paths may start with ~ (the measuring machine's home).
    for k in ("repo", "workdir", "target_dir", "checkpoint", "trend", "lock", "measure_cli", "merge_log"):
        if isinstance(cfg.get(k), str):
            cfg[k] = os.path.expanduser(cfg[k])
    for k in ("pdfs", "submittals", "specs", "cost_db"):
        v = cfg.get("job", {}).get(k)
        if isinstance(v, str):
            cfg["job"][k] = os.path.expanduser(v).replace("\\", "/")
    work = os.path.abspath(cfg.get("workdir", "accuracytrend-work"))
    os.makedirs(work, exist_ok=True)
    trend = cfg["trend"]
    repo = cfg.get("repo")
    with Lock(cfg.get("lock")):
        commit = a.commit
        if repo:
            hub, branch = cfg.get("hub", "origin"), cfg.get("branch", "next")
            subprocess.run(["git", "-C", repo, "fetch", "-q", hub, f"+refs/heads/{branch}:refs/remotes/{hub}/{branch}"], check=False)
            commit = git(repo, "rev-parse", "--verify", f"{commit or f'{hub}/{branch}'}^{{commit}}")
        if not commit:
            sys.exit("name --commit (no repo in the config to resolve it)")
        prev = last_row(trend)
        if a.if_new and prev and same_commit(prev.get("commit"), commit):
            log(f"{commit[:10]} is already the trend's newest row: nothing to measure")
            return 0
        cli = a.cli or (build_cli(cfg, commit, work) if repo else None)
        if not cli:
            sys.exit("name --cli (no repo in the config to build one)")
        if a.model:
            built, secs, peak = a.model, a.job_s, a.peak_mb
        else:
            built, secs, peak = run_job(cfg, cli, commit, work)
        # The commits between the build before and this one, and the merger's batch log.
        commits = os.path.join(work, "commits.txt")
        with open(commits, "w", encoding="utf-8") as f:
            if repo and prev and prev.get("commit"):
                try:
                    f.write(git(repo, "log", "--oneline", "--first-parent", f"{prev['commit']}..{commit}") + "\n")
                except subprocess.CalledProcessError:
                    pass
        merge_log = cfg.get("merge_log")
        if cfg.get("fetch_merge_log"):
            merge_log = os.path.join(work, "merge.log")
            subprocess.run(["scp", "-q", "-o", "BatchMode=yes", cfg["fetch_merge_log"], merge_log], check=False)
        mcfg = os.path.join(work, "measure-config.json")
        json.dump({k: cfg[k] for k in ("rooms", "skip_rules", "label", "last") if k in cfg}, open(mcfg, "w", encoding="utf-8"))
        base = os.path.splitext(trend)[0]
        args = [cfg.get("measure_cli") or cli, "accuracy", built, "--config", mcfg, "--commit", commit, "--trend", trend,
                "--commits", commits, "--row", os.path.join(work, f"row-{commit[:10]}.json"), "--table", base + ".md",
                "--svg", base + ".svg", "--force"]
        if secs is not None:
            args += ["--job-s", f"{secs:.1f}"]
        if peak:
            args += ["--peak-mb", str(int(peak))]
        if merge_log and os.path.exists(merge_log):
            args += ["--merge-log", merge_log]
        log("measuring " + built)
        code = subprocess.run(args).returncode
        if code not in (0, 2):
            log(f"the measure failed (exit {code})")
            return 1
        if cfg.get("copy_to") and not a.no_copy:
            for f in (trend, base + ".md", base + ".svg"):
                if os.path.exists(f):
                    subprocess.run(["scp", "-q", "-o", "BatchMode=yes", f, cfg["copy_to"]], check=False)
        log("REGRESSION: see the findings above" if code == 2 else "no regression")
        return code


if __name__ == "__main__":
    sys.exit(main())
