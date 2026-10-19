# Privacy

CADaddy has no telemetry, no analytics and no account. Your drawings stay on your computer unless you
send them somewhere yourself. This page lists everything CADaddy does over the network; a test
(`crates/ui-egui/tests/no_network.rs`) fails if the code makes any other network call.

## What goes over the network

| What | When | Where to | What is sent | Turn it off |
|---|---|---|---|---|
| **Update check** (`apps/cadaddy/src/updater.rs`) | At most once a day after start, and when you choose Help ▸ Check for Updates | `api.github.com` (the latest CADaddy release) | An ordinary HTTPS request for the release list, sent through the system's `curl`, so the usual request details (your IP address, curl's user agent) go to GitHub. Nothing about you or your drawings | Options ▸ User Preferences ▸ Check for updates automatically (Help ▸ Check for Updates still works when you ask) |
| **Update download** (same file) | Only when you click Update | `github.com/AICodeForgedev/cadaddy/releases/download/…` | The download request; the file's SHA-256 is checked before it installs | Don't click Update |
| **MCP server** (`crates/mcp/src/backend.rs`) | When an AI assistant runs `cadaddy-cli mcp --connect` | The CADaddy app at the address given, `127.0.0.1` (this computer) by default | The assistant's commands, to your own copy of CADaddy | Don't start it |
| **Web app** (`apps/cadaddy-web/src/upload.rs`, `apps/cadaddy-web/src/selftest.rs`) | When the web app is opened with a walkthrough link (`?walk=<address>`), or with `?selftest` | The address in the link: the web app's own site, or another site that allows it (the browser enforces that) | A request for that file; the self-test's results go back to the site the web app came from | Don't open such links |

## What listens

The **control channel** and the **agent port** (`apps/cadaddy/src/control_server.rs`) listen on
`127.0.0.1` only: programs on this computer (an AI assistant, a script) can drive CADaddy; nothing on
the network can reach them. The agent port can be turned off in Options ▸ User Preferences (Let AI agents
control CADaddy), and agents' saves over files and closes with unsaved changes wait for you to confirm.

## What opens your browser

Links you click open in your web browser; CADaddy itself doesn't fetch them:

- Help, Report an Idea and Donate links (`aicodeforge.dev/cadaddy`);
- the release notes (`github.com/AICodeForgedev/cadaddy/releases`);
- contributors' profiles in About;
- hyperlinks stored in a drawing, which you open with Ctrl+click.

## Report a Problem

Help ▸ Report a Problem makes a ZIP file **on your computer**; nothing is sent. You choose whether to
email it. The dialog lists every file in the report before you save it.

- **Always in it:**
  - the log of this run and the two before it;
  - the last commands (command names only, unless you include names);
  - the versions, the operating system and the graphics adapter.
- **Only when you tick it:**
  - a summary of the open drawings: counts only, no geometry;
  - names: file paths, layer, block, style and layout names, and what you typed;
  - the active drawing itself, as a DXF file.
- **Never in it:** who you are. Your account name, your home folder (in any form a path may take) and
  your computer's name are replaced by `<user>`, `~` and `<computer>` in every file, logs included,
  even when you include names.

## Files CADaddy keeps

Preferences, the logs, autosaves, the activity log beside a drawing (when ACTIVITYLOG is on) and
downloaded updates stay in CADaddy's folders on your computer. Uninstalling removes the program;
your settings folder stays until you delete it.

## Questions

feedback@aicodeforge.dev
