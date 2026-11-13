# Projects and the transmittal manifest

A project is a folder with a `project.json` in it. This page is the schema of that file, of the
`manifest.json` that `project.import_manifest` reads, and of what shelving does to a folder.

## Folder layout

```
<project>/
  project.json        the project's manifest (below)
  *.cdy, *.dxf, ...   drawings listed in project.json
  rules.toml, overrides.json, types/, review/ ...   support files (kept as they are)
  cache/ tiles/ temp/ indexes/    rebuildable; dropped when the project is shelved
```

The projects on a machine are listed in `projects.json`, the company standards in
`standards.json`, both in the settings folder (`CADADDY_CONFIG_DIR`, else the user's settings
folder).

## `project.json`

| field | meaning |
|---|---|
| `name` | display name |
| `created`, `modified` | ISO 8601 UTC times (`2026-10-31T14:05:09Z`) |
| `drawings[]` | entries below |
| `standards[]` | this project's own values: `{domain, key, value, source}`; they override the company's |
| `shelved` | `true` while the files are packed in the archive |
| `archive`, `archivedFiles` | the archive's name (`project.shelved`) and how many files it holds |
| `queue[]` | paths a manifest import added or changed, still to look at |

A drawing entry: `path` (relative to the project folder), `title`, `discipline`, `kind`,
`revision`, `date`, `status` (`active` / `shelved` / `archived`: what the project does with it),
`reviewStatus` (a submittal's or RFI's status, below), `links`, `notes` (yours; an import never
writes them), `contentHash`, `fileSize`, `fromManifest`.

## Standards inheritance

`project.company.set` writes the company library. `project.standards.list` returns the effective
standards: each company value, replaced by the project's own where the project sets one. Every
entry has `source` = `company` or `project`; an overridden one also has `overrides` (the company
value). `project.standards.remove` drops the project's override. Changing a company value moves
every project that does not override it.

## Shelve and restore

`project.shelve_project` drops the top-level `cache`, `tiles`, `temp` and `indexes` folders,
packs every other file into one deflate archive `project.shelved` (each file with its length and a
hash; the archive is read back and checked before anything is removed), deletes the packed files
and leaves a stub `project.json` (`shelved: true`). `project.restore_project` (or opening the
project) checks the whole archive, then writes every file back as it was. A damaged archive is
refused and nothing is written. A folder with a symbolic link in it cannot be shelved.

## `manifest.json` (a transmittal)

```json
{
  "name": "Tower - issued set 4",
  "files": [
    {"path": "A-101.pdf", "kind": "sheet", "title": "Level 1 plan", "discipline": "Architectural",
     "revision": "C", "date": "2026-10-01"},
    {"path": "submittals/sub-03.pdf", "kind": "submittal", "status": "approved as noted",
     "links": {"spec": ["26 05 33"]}},
    {"path": "rfi/rfi-12.pdf", "kind": "rfi", "status": "answered", "links": {"sheet": ["E-201"]}}
  ]
}
```

- `files` is required, at most 10,000 entries; each is an object.
- `path` is required: a relative path inside the manifest's folder. `..`, a leading `/`, a drive
  (`C:`), a backslash, `.` or empty parts are refused, and so is a repeated path. A manifest with
  such a path imports nothing.
- `kind`: `drawing`, `sheet`, `spec`, `submittal`, `rfi`, `asi`, `addendum`, `photo`, `markup`,
  `bulletin` or `other` (default `drawing`).
- `status` is for two kinds only: a submittal's `approved`, `approved as noted`,
  `revise and resubmit` or `rejected`; an RFI's `open` or `answered`. Any case, and `_` or `-` for
  a space, are accepted; it is stored in its own field `reviewStatus` in the canonical spelling.
  Any other value, or a status on another kind, is an error.
- `title`, `discipline`, `revision`, `date` are strings of at most 4,096 bytes; `links` is an
  object of string arrays. A value of the wrong type is an error that names the entry and field.
- `manifest.json` is read up to 64 MB.
- `sourceId` (or `source_id`) and `pulled` (an ISO 8601 time) on a file: its id at the source and
  when it was pulled; a file without `pulled` takes the `source` block's.

### The document source (`source`)

```json
{
  "name": "ASI 1",
  "source": {"kind": "platform", "platform": "Example CM", "reference": "P-100",
             "pulled": "2026-11-13T09:30:00Z"},
  "files": [ ... ]
}
```

A transmittal may say where its documents came from. `kind` is required: `platform` (a
construction-management platform), `shared_drive`, `plan_room`, `email` or `none` (the kinds and
their aliases are rule data, `crates/engine/src/cmd/docsource.json`). `platform` is the place's own
name, `reference` the project reference the puller understands, `pulled` when it was pulled (none:
the import time), `maxAgeHours` how old a pull may get before CADaddy asks again (default 24). The
import keeps them in `project.json` as `source` (`kind`, `platform`, `reference`, `maxAgeHours`,
`lastPulled`, `lastChecked`, `issue` = the manifest's `name`); `project.source {kind?, platform?,
reference?, max_age_hours?}` reads or sets them.

CADaddy never signs in anywhere and fetches nothing: the pull is done outside it (an agent, a
connector or a script with the owner's own read-only access). It asks:

- a guided job, at its start (and on resume) when the last pull or check is older than the age or
  never happened, and again before its takeoff and estimate: a question `Pull the latest
  documents from …` whose evidence lists what to pull (the current drawings, addenda and ASIs,
  RFIs with responses, submittals with status, specifications), the project reference, changed
  since when, and how to hand it back (this folder, this manifest). `job.answer` takes `pulled`
  (or `{answer: "pulled", manifest}`), `nothing newer` or `skip` (the default: an unattended job
  goes on and its report says the latest was not confirmed). A source of kind `none` is never
  asked;
- a finding newer documents could answer (documents that disagree, a missing sheet or schedule,
  an unanswered callout, a product with no submittal) carries `check_source` in `job.worklist`:
  the request to check for a newer sheet, ASI, RFI response or submittal, with its sheets and
  specification sections;
- after `pulled`, only the sheets whose files are new or changed are read again (the others keep
  their reading) and the job runs on from classify; a new or changed specification or submittal
  runs the build again; RFIs and ASIs are listed on the findings whose sheets they name. A finding
  the re-read no longer raises is closed naming the documents that answered it (`job.report`
  `documents.closed_findings`); answers and corrections carry over;
- `job.report`, `job.status` and the takeoff, BOM, estimate, bid, AR export and briefing answers
  carry `documents`: the issue they were built from, when the source was last checked and a line
  in plain words; on stale documents those that produce something also carry `pull_request`.

`project.validate_manifest {path}` checks a file without importing it.

### Re-import

`project.import_manifest {path}` on a folder whose project exists compares the manifest with what
the project holds and answers `files`:

- `added`: paths new to the project;
- `changed`: `[{path, fields}]` for files whose manifest data or bytes differ; `fields` lists
  `content`, `status`, `revision`, `date`, `discipline`, `kind`, `title`, `links`. A status that
  moved with identical bytes counts;
- `removed`: entries an earlier import made that the manifest no longer lists (dropped from the
  project; the file itself is untouched);
- `missing`: listed files that are not in the folder (kept; they can arrive later).

Only `added` and `changed` paths are queued (`queued`, and `queue` in `project.info`); a removed
path leaves the queue. Entries added by hand (`project.add`) are never removed by an import.
