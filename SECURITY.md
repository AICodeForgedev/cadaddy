# Security

Thank you for helping keep CADaddy and the people who use it safe.

## Supported versions

Security fixes go into the **latest release** only. CADaddy checks for updates once a day (Help ▸ Check
for Updates checks now) and an installed copy updates itself when it closes, so the latest release is
the one to use and to test.

| Version | Security fixes |
|---|---|
| The latest release ([releases](https://github.com/AICodeForgedev/cadaddy/releases)) | Yes |
| Older releases | No: please update |

## Reporting a problem

Please **don't** open a public issue or post the details anywhere public. Email
**feedback@aicodeforge.dev** with a subject starting **SECURITY:** and include:

- what the problem is and what someone could do with it;
- the CADaddy version (Help ▸ About) and the operating system;
- the steps or a file that shows it. Use a made-up drawing: never send drawings that belong to you or
  anyone else's work;
- whether, and how, you'd like to be credited.

## What happens next

- We'll reply within **7 days** to say we have it, and tell you what we think of it.
- We'll keep you told as we work on a fix. Most fixes ship in the next release; a serious problem gets a
  release of its own.
- We'll credit you in the release notes when the fix ships, if you'd like that.
- Please give us **90 days**, or until the fix is released if that's sooner, before you publish the details.
  If we need longer we'll explain why.

There is **no bug bounty**: CADaddy is free and run without money for one. We're grateful for every report
all the same.

## What's in scope

- The CADaddy desktop app (Windows, macOS, Linux) and its installers.
- `cadaddy-cli` and its MCP server (`CADaddy.mcpb`).
- The web app.
- How CADaddy updates itself, and what it reads: DWG, DXF, .cdy, PDF, SVG, images, fonts, AutoLISP
  and script files, walkthrough packages.
- The control channel and agent port, which must only be reachable from your own computer.

Especially welcome: a file that makes CADaddy run code, write where it shouldn't or reach the network; a
way past the agent safety rails or the control channel's local-only rule; anything that lets an update be
tampered with.

Out of scope: the aicodeforge.dev and GitHub websites themselves, social engineering, and denial of service
that needs physical access to the computer. A crash from a damaged file is a bug we want to hear about too,
through **Help ▸ Report a Bug**, unless it looks exploitable.

## How CADaddy protects you

- No telemetry. The only network use is the update check and what you ask for (see PRIVACY.md).
- Updates are only taken from the official releases, and each download is checked against its SHA-256
  before it installs.
- Every file reader treats its input as hostile: sizes, counts and nesting are capped, and the readers are
  fuzzed.
- The control channel and agent port listen on 127.0.0.1 only. Agents can't save over your files or close
  drawings with unsaved changes without your OK, and only you can turn that off.
- The dependencies are checked against the RustSec advisory database and for their licences.
