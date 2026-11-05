# The accuracy trend

Accuracy is measured on every build, not when someone remembers to. One build of a job gives one
row: the same checks Validate runs before an agent judges anything, so a row costs no agent usage.
Each new row is compared with the row before it. A step back is reported the day it lands, with
the merged branches that could have caused it.

## What one row holds

`validate.measure` measures the open, built model. `cadaddy-cli accuracy` wraps it. The row's
`metrics` is a flat map of numbers, keyed so that two rows compare key by key:

| metrics | what |
|---|---|
| `gate_a.*` | Gate A, worked out the way the Gate A script works it from the building prescreen and the model's areas: `host_pass_pct`, `host_fail_far` (more than 12 in off a wall face), `height_pass_pct`, `height_measured`, `height_unmeasured`, `outside_building` |
| `coverage.*` | G-COVERAGE: `kinds_defined`, `fail`, `pass`, `unsure`, `not_marked`, `zero_in_model`, `unexplained_groups`. The failing kinds are named in `detail.coverage_failing`. |
| `building.<rule>.*` | every prescreen rule's verdict counts over the building, with `accuracy_pct` (pass / (pass + fail + missing + extra)) and `problems` (fail + missing + extra + conflict); `building.all.*` sums every rule |
| `rooms.<rule>.*`, `rooms.found` | the same over the fixed Validate rooms the config lists, summed. Per room it is in `detail.rooms.by_room`; rooms the model no longer has are in `detail.rooms.missing`. |
| `takeoff.<trade>/<system>.<unit>`, `takeoff.<trade>.<unit>` | the model takeoff's totals by trade and system (`bim.takeoff` by system) |
| `elements.<kind>`, `elements.total` | the model's elements by kind (a device by its own kind) |
| `build.job_s`, `build.peak_mb` | the job's wall time and peak memory as the script measured them; `build.measure_s` is the measure's own time. The job's stage times are in `detail.stages_s`. |

The row is keyed by `commit`, with `at` (UTC) and `label`, the job or machine. The fixed rooms
and the label come from the measuring machine's config, never from code.

## The regression rule

`validate.trend` appends the row to the trend file. The file is JSON lines, one row per line, and
is only ever appended to. The command then compares the newest row with the one before it, using
the rules data in `crates/engine/src/cmd/validate_trend.json`. Each metric pattern has a better
direction (`higher`, `lower`, or `stable` for counts and quantities) and a tolerance: `abs`, or
`share` of the earlier value, whichever is larger.

A metric that is worse beyond its tolerance is a **REGRESSION** finding. The finding names:

- the metric and its two values;
- the two commits;
- the commits between them (`--commits`: `git log --oneline` of the range);
- with the merger's batch log (`--merge-log`), the batches that merged between the two builds and
  their branches, as the suspects.

A metric the earlier row had and the newest row lacks also regresses ("not measured now"), unless its rule is `optional`: the job's time and peak memory, which the caller gives.
A `stable` count that falls beyond its tolerance (elements or quantities lost) is a regression.
One that rises is a **CHANGE** finding: the merge built something new, so it gets explained, but
it is not a step back. If the new elements are wrong, the accuracy metrics regress beside it.

`cadaddy-cli accuracy` exits with status **2** when anything regressed. Changes alone don't set it.

The table of the last N rows (`--last`, default 10) is printed and written as Markdown (`--table`).
A value that regressed is marked `(!)`, and a count that changed `(~)`. A small SVG plot of the percentages is written too
(`--svg`).

## Running it

One measure of an already built model:

```
cadaddy-cli accuracy built.cdy --config trend-config.json --commit 1a2b3c4 --job-s 1800 --peak-mb 3100 \
    --trend trend.jsonl --merge-log merge.log --commits commits.txt --table trend.md --svg trend.svg
```

The whole loop for one commit is `scripts/accuracytrend.py`. It does six things:

1. It takes the job-run lock, so only one job runs on the machine at a time.
2. It builds that commit's CLI.
3. It runs the job on a fresh copy of the checkpoint, under the 2 h limit. A run that reaches the
   limit is reported, not restarted.
4. It measures the built model.
5. It appends the row to the trend and compares it with the row before.
6. It copies the trend file, the table and the plot to the hub.

```
python3 -I scripts/accuracytrend.py --config ~/cadwork/trend-config.json --commit <next commit>
```

The config is private to the measuring machine, because it names the job's files. Never commit
one. `scripts/accuracytrend.example.json` shows its keys with made-up paths.

## The loop hook

There are two ways to run it after each merger batch:

- **The coordinator** runs it after each batch on the measuring machine:
  `ssh <machine> 'python3 -I ~/repos/ourcad/scripts/accuracytrend.py --config ~/cadwork/trend-config.json --commit <the batch's next>'`.
- **A cron job** on the measuring machine measures each new `next` by itself. With `--if-new`, a
  run whose commit is already the trend's newest row stops at once, so a short interval costs
  nothing:

  ```
  */30 * * * * cd ~/repos/ourcad && python3 -I scripts/accuracytrend.py --config ~/cadwork/trend-config.json --if-new >> ~/cadwork/accuracytrend.log 2>&1
  ```

Either way, only one job run happens at a time. The script's `lock` file is the machine's
job-run lock, and a run that finds it held exits with status 3. If the machine uses `flock`
instead, wrap the cron line in `flock -n <lock>` and leave `lock` out of the config.

Exit statuses: 0 measured or nothing new, 2 regressed, 3 lock held, 4 the job reached the time
limit, 1 anything else.
