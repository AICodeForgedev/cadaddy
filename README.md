<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/cadaddy-logo-dark.png">
    <img alt="CADaddy" src="docs/cadaddy-logo-light.png" width="420">
  </picture>
</p>

<p align="center">
  <b>Free CAD software for Windows and Mac: a fast, free AutoCAD alternative for 2D drafting.</b><br>
  The AutoCAD workflow you already know (command line, object snaps, layers, dimensions,
  hatches, blocks and xrefs) with real DWG and DXF files. No ads, no subscription, no account.
</p>

<p align="center">
  <a href="https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Windows.msi"><b>⬇ Download for Windows</b></a>
  &nbsp;·&nbsp;
  <a href="https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy-Mac.dmg"><b>⬇ Download for Mac</b></a>
  &nbsp;·&nbsp;
  <a href="https://github.com/AICodeForgedev/cadaddy/releases">All versions</a>
</p>

<p align="center"><a href="https://aicodeforge.dev">aicodeforge.dev</a> · also from AICodeForge: <a href="https://github.com/AICodeForgedev/redline"><b>Redline</b></a>, a free PDF editor for construction drawings (markup, measure, count, sign)</p>

## Install

**Windows 10 or 11 (64-bit)**
1. Download **CADaddy-Windows.msi** and open it.
2. Click **Next → Install → Finish**. No administrator password needed.
3. If Windows says *"Windows protected your PC"*, click **More info → Run anyway**. The installer isn't code-signed yet.

**Mac (Apple chip or Intel, macOS 11 or newer)**
1. Download **CADaddy-Mac.dmg** and open it.
2. Drag **CADaddy** onto **Applications**.
3. The first time, if your Mac says it *"could not verify"* CADaddy, open **System Settings ▸ Privacy & Security** and click **Open Anyway**. The app isn't notarized yet.

## What it does

- Opens and saves **DWG** and **DXF** drawings.
- AutoCAD-style ribbon and command line, with object snaps, layers, dimensions, hatches, blocks, dynamic blocks, xrefs, layouts and plotting.
- **Walk mode** (View ▸ Walk): walk through a building in 3D, straight from its floor plans.
- Converts PDF drawings to DWG.

## For AI agents

CADaddy is built to be driven by AI assistants. **One click for Claude Desktop:** download
[CADaddy.mcpb](https://github.com/AICodeForgedev/cadaddy/releases/latest/download/CADaddy.mcpb) and open it.
It's also in the official [MCP Registry](https://registry.modelcontextprotocol.io) as `io.github.AICodeForgedev/cadaddy`.

Every one of its 400+ commands can be called as JSON, and `cadaddy-cli` (installed with the app)
includes a Model Context Protocol (MCP) server:

```sh
cadaddy-cli mcp                                  # headless MCP server
cadaddy --control 7800                           # or drive the open app:
cadaddy-cli mcp --connect 127.0.0.1:7800
cadaddy-cli convert plan.dwg plan.pdf            # headless conversion
```

Example calls: `open {"path": "plan.dwg"}`, `line {"points": [[0,0],[100,0]]}`,
`dimlinear {"p1": [0,0], "p2": [100,0], "at": [50,-10]}`, `saveas {"path": "plan.dwg"}`.
A plain-text summary for language models is at [aicodeforge.dev/llms.txt](https://aicodeforge.dev/llms.txt).

## Feedback and support

- **Found a bug or want a feature?** In CADaddy, use **Help ▸ Report a Bug** or **Request a Feature**, fill in the [feedback form](https://aicodeforge.dev/cadaddy/feedback), or email **feedback@aicodeforge.dev**.
- **Like it?** CADaddy is free, and it stays that way. A donation pays for the AI time that builds the next features: **[Donate to CADaddy](https://aicodeforge.dev/donate)**.

## License

CADaddy is licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. It is
based on [CADCraft](https://github.com/storytold/cadcraft) by the ArtCraft team and the CADCraft
contributors. Required notices, including the third-party DWG library, are in [NOTICE](NOTICE).

<sub>Autodesk, AutoCAD and DWG are trademarks of Autodesk, Inc. CADaddy is an independent project and is not affiliated with, sponsored by or endorsed by Autodesk, Inc.</sub>
