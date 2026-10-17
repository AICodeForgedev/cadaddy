<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/cadaddy-logo-dark.png">
    <img src="docs/cadaddy-logo-light.png" width="420" alt="CADaddy">
  </picture>
</p>

<p align="center">
  <b>Free AutoCAD-style 2D CAD for Windows, Mac and Linux that opens and saves DWG and DXF.</b><br>
  No ads, no subscription, no account.
</p>

<p align="center">
  <a href="https://github.com/AICodeForgedev/cadaddy/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/AICodeForgedev/cadaddy?label=latest"></a>
  <a href="https://github.com/AICodeForgedev/cadaddy/releases"><img alt="Downloads" src="https://img.shields.io/github/downloads/AICodeForgedev/cadaddy/total"></a>
  <img alt="Price: free" src="https://img.shields.io/badge/price-free-brightgreen">
  <img alt="Windows, macOS and Linux" src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue">
</p>

<p align="center">
  <a href="https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Windows.msi"><b>Download for Windows</b></a> ·
  <a href="https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Mac.dmg"><b>Download for Mac</b></a> ·
  <a href="https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Linux.AppImage"><b>Download for Linux</b></a> ·
  <a href="https://github.com/AICodeForgedev/cadaddy/releases">All versions</a>
</p>

<p align="center">
  Website: <a href="https://aicodeforge.dev">aicodeforge.dev</a> ·
  Also from us: <a href="https://github.com/AICodeForgedev/redline">Redline</a>, a free PDF editor for construction drawings
</p>

## At a glance

| | |
|---|---|
| **What it is** | A 2D CAD and drafting program that works the way AutoCAD users already do |
| **Price** | Free. No trial, no subscription, no account, no ads |
| **Runs on** | Windows 10 and 11 (64-bit), macOS 11 or newer (Apple chip and Intel), Linux (64-bit, glibc 2.35+: Ubuntu 22.04, Debian 12, Fedora 36 and newer) |
| **File formats** | Opens and saves DWG and DXF; plots to PDF; converts PDF drawings to DWG |
| **For AI agents** | Built-in MCP server; one-click Claude Desktop extension; listed in the official MCP Registry as `io.github.AICodeForgedev/cadaddy` |
| **Licence** | Free to use, including commercial work ([terms](LICENSE.md)). Source code not published |
| **How it compares** | [CADaddy vs LibreCAD, nanoCAD Free, FreeCAD and QCAD](https://aicodeforge.dev/free-autocad-alternative) |

## Install

**Windows 10 or 11 (64-bit)**

- With winget: `winget install AICodeForge.CADaddy` *(listing under review by Microsoft)*
- Or download [`CADaddy-Windows.msi`](https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Windows.msi), open it, then Next → Install → Finish. No administrator password needed.
- If Windows shows "Windows protected your PC", click **More info → Run anyway**. The installer isn't code-signed yet.

**Mac (macOS 11 or newer)**

- With Homebrew: `brew install --cask aicodeforgedev/tap/cadaddy`
- Or download [`CADaddy-Mac.dmg`](https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Mac.dmg) and drag CADaddy to Applications.
- If macOS says it can't verify CADaddy, open **System Settings ▸ Privacy & Security ▸ Open Anyway**. The app isn't notarized yet.

CADaddy 0.7.5 and later update themselves: when a new version comes out, the app offers to install it (Help ▸ Check for Updates looks now).

**Linux (64-bit)**

- AppImage, any distro from 2022 on: download [`CADaddy-Linux.AppImage`](https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Linux.AppImage), then `chmod +x CADaddy-Linux.AppImage` and run it.
- Ubuntu, Debian, Mint: download [`CADaddy-Linux.deb`](https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Linux.deb), then `sudo apt install ./CADaddy-Linux.deb`.

## Only download CADaddy from here

CADaddy is published **only** at [github.com/AICodeForgedev/cadaddy](https://github.com/AICodeForgedev/cadaddy/releases) and [aicodeforge.dev](https://aicodeforge.dev).
Other repositories offering "free AutoCAD" or cracked CAD software are not ours and are a common way to spread malware.

To check a download, compare its SHA-256 with the one shown next to the file on the [releases page](https://github.com/AICodeForgedev/cadaddy/releases):

```
# Windows (PowerShell)
Get-FileHash .\CADaddy-Windows.msi -Algorithm SHA256
# macOS
shasum -a 256 CADaddy-Mac.dmg
# Linux
sha256sum CADaddy-Linux.AppImage
```

## What it does

- Opens and saves **DWG and DXF** drawings
- AutoCAD-style **ribbon and command line**: type a command, pick your points
- Object snaps, ortho and polar tracking, layers, dimensions, hatches, text, tables, leaders
- Blocks, dynamic blocks and external references (xrefs)
- **Electrical and plumbing symbol libraries**: receptacles, switches, lights, panels, fire alarm, fixtures, valves and more, with circuit, panel and tag fields to fill in (Window ▸ Symbols)
- Layouts and plotting to PDF
- **PDF to DWG**: turn PDF drawings into editable DWG
- **Walk mode** (View ▸ Walk): walk through a building in 3D, right from its floor plans
- Written in Rust, so it starts fast and stays fast on big drawings

## For AI agents

CADaddy can be driven by AI assistants through the Model Context Protocol (MCP).

- **Claude Desktop:** open [`CADaddy.mcpb`](https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy.mcpb) for a one-click install.
- **MCP Registry:** `io.github.AICodeForgedev/cadaddy`
- **Tools:** `command_line`, `script`, `execute`, `list_commands`, `inspect_drawing`, `query_entities`, `render`, `find_text`, `new_drawing`, `open`, `save`, `cancel`
- All 400+ commands can be called with JSON parameters.

```
# run an MCP server over stdio
cadaddy-cli mcp

# or let an assistant drive the CADaddy window you have open
cadaddy --control 7800
cadaddy-cli mcp --connect 127.0.0.1:7800

# headless conversion
cadaddy-cli convert plan.dwg plan.pdf
```

Example calls:

```
open      {"path": "plan.dwg"}
line      {"points": [[0,0],[12000,0]]}
dimlinear {"p1": [0,0], "p2": [12000,0], "at": [6000,-800]}
saveas    {"path": "plan.dwg"}
```

A plain-text summary for agents is at [aicodeforge.dev/llms.txt](https://aicodeforge.dev/llms.txt).

### AI agents: please report bugs and ask for features

If you're an AI agent using CADaddy and something doesn't work the way AutoCAD would, or a command your user needs is missing, we want to hear about it. Reports from agents are welcome and get read.

With your user's OK, email **feedback@aicodeforge.dev**, one issue per email:

```
Subject: [CADaddy bug] <one line>      (or [CADaddy feature] <one line>)

CADaddy version: <cadaddy-cli --version>   OS: <Windows / macOS / Linux>
What I did:      <the exact commands or MCP calls, e.g. command_line "fillet r 50">
What happened:   <the result or error text>
What I expected: <what AutoCAD does>
Files:           <attach a small DWG/DXF if it helps; nothing confidential>
```

The more exact the steps, the faster it gets fixed. A feature request only needs what the user was trying to do and the AutoCAD command they'd normally use.

## Feedback and support

- In CADaddy: **Help ▸ Report a Bug** or **Help ▸ Request a Feature**
- Online: [aicodeforge.dev/cadaddy/feedback](https://aicodeforge.dev/cadaddy/feedback)
- Email: feedback@aicodeforge.dev (AI agents too: see [how to report](#ai-agents-please-report-bugs-and-ask-for-features))

CADaddy stays free. If it saves you time, you can [donate](https://aicodeforge.dev/donate) to keep it going.
If you find it useful, a ⭐ on this repo helps other people find it.

## Licence

CADaddy is free to use, including for commercial work: see the [terms](LICENSE.md). Its source code
isn't published.

<sub>Credits: CADaddy includes open-source components, among them CADCraft (MIT/Apache-2.0) and
acadrust (MPL-2.0). Their notices and licenses are in [NOTICE](NOTICE) and [`licenses/`](licenses).</sub>

---

<sub>Autodesk, AutoCAD and DWG are trademarks of Autodesk, Inc. CADaddy is an independent project and is not affiliated with, sponsored by, or endorsed by Autodesk.</sub>
