# For AI Agents

CADaddy is designed to be driven by AI agents. It exposes its full command set
through the Model Context Protocol (MCP) and a TCP JSON control channel.

## MCP setup in Claude Desktop

Add CADaddy to your Claude Desktop configuration
(`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS,
`%APPDATA%\Claude\claude_desktop_config.json` on Windows):

```json
{
  "mcpServers": {
    "cadaddy": {
      "command": "cadaddy-cli",
      "args": ["mcp"]
    }
  }
}
```

This starts `cadaddy-cli mcp`, which connects to the running CADaddy desktop app
on port 47820 (or runs headless if the app is not open).

### Connecting to a specific instance

```json
{
  "mcpServers": {
    "cadaddy": {
      "command": "cadaddy-cli",
      "args": ["mcp", "--connect", "127.0.0.1:47820"]
    }
  }
}
```

### Headless mode

```json
{
  "mcpServers": {
    "cadaddy": {
      "command": "cadaddy-cli",
      "args": ["mcp", "--headless"]
    }
  }
}
```

No desktop window; the agent works on drawings entirely through MCP tools.

## MCP setup in Claude Code

Add to your project's `.mcp.json` or global MCP settings:

```json
{
  "mcpServers": {
    "cadaddy": {
      "command": "cadaddy-cli",
      "args": ["mcp"]
    }
  }
}
```

## Available MCP tools

CADaddy exposes these tools over MCP:

### Drawing commands

| Tool | Description |
|:-----|:------------|
| `command_line` | Type at the command line (spaces separate tokens, like a user typing) |
| `script` | Run a multi-line script |
| `execute` | Run a command with JSON parameters (no dialogs) |
| `list_commands` | List available commands with optional filter |

### Drawing inspection

| Tool | Description |
|:-----|:------------|
| `inspect_drawing` | Counts, extents, layers, styles, blocks, selection, undo stack |
| `query_entities` | Filter and retrieve entities with geometry (paginated) |
| `find_text` | Search TEXT, MTEXT, attributes and multileaders by content |
| `get_variable` | Read a system variable |
| `set_variable` | Set a system variable |
| `get_prompt` | Current command-line prompt state |
| `wait_until_idle` | Block until no command is active |

### File operations

| Tool | Description |
|:-----|:------------|
| `new_drawing` | Create a blank drawing |
| `open` | Open a file |
| `save` | Save the active drawing |
| `close_drawing` | Close a drawing (with save/discard option) |
| `close_all` | Close all drawings |
| `list_drawings` | List open drawings |
| `switch_drawing` | Switch to another open drawing |

### Visual feedback

| Tool | Description |
|:-----|:------------|
| `render` | Export the current view as PNG (returns the image) |
| `screenshot` | Screenshot of the application window |
| `ui_inspect` | Inspect the UI state (panels, dialogs) |
| `ui_elements` | List visible UI elements |
| `ui_click` | Click a UI element by label or position |

### Editing

| Tool | Description |
|:-----|:------------|
| `select` | Select entities by handles, window, or all |
| `undo` | Undo the last command |
| `redo` | Redo |
| `cancel` | Cancel the active command |
| `plot` | Export to PDF |

### AutoLISP

| Tool | Description |
|:-----|:------------|
| `run_lisp` | Evaluate an AutoLISP expression |

### Application control

| Tool | Description |
|:-----|:------------|
| `answer_close_prompt` | Respond to a "save changes?" prompt |
| `quit_app` | Quit the application |

## MCP resources

| URI | Description |
|:----|:------------|
| `cadaddy://drawing` | The current drawing as JSON |
| `cadaddy://commands` | Full command catalog |

## Example agent session

Here is a typical sequence an agent might use:

```
1. new_drawing {}
2. execute {"command": "rectang", "params": {"p1": [0,0], "p2": [100,60]}}
3. execute {"command": "circle", "params": {"center": [50,30], "radius": 10}}
4. execute {"command": "dimlinear", "params": {"p1": [0,0], "p2": [100,0], "at": [50,-8]}}
5. inspect_drawing {}
6. render {}
7. save {"path": "Sample Floor Plan.cdy"}
```

## TCP JSON control channel

For programmatic control without MCP, start CADaddy with a control port:

```sh
cadaddy --control 47820
```

Connect over TCP and send JSON-line requests. First authenticate with the token
written to `control/47820.token` in the settings directory:

```json
{"id": 0, "method": "auth", "params": {"token": "..."}}
```

Then send commands:

```json
{"id": 1, "method": "engine.execute", "params": {"command": "line", "params": {"points": [[0,0],[10,0]]}}}
{"id": 2, "method": "ui.screenshot", "params": {"path": "/tmp/shot.png"}}
```

The full protocol is documented in `docs/control-protocol.md`.

## Safety rails

CADaddy enforces safety limits on the control channel and MCP:

- **Authentication required.** Every connection must present a valid token.
- **Rate limiting.** 500 requests/second average, burst to 2,000.
- **Request size.** 256 MiB maximum per request.
- **Idle timeout.** Connections idle for 10 minutes are closed.
- **No file execution.** Hyperlinks, xrefs and packages pointing at executables,
  UNC paths or `file://` programs prompt the user before opening.
- **AutoLISP sandboxing.** Scripts outside trusted locations do not run
  automatically.
- **Read-only by default.** MCP tools that modify the drawing require explicit
  tool-use approval from the agent framework.

## CLI for batch scripting

`cadaddy-cli` can run scripts without a window:

```sh
cadaddy-cli run --script 'RECTANG 0,0 100,60\nCIRCLE 50,30 10\n' --export out.png
cadaddy-cli run --sample --export sample.pdf
cadaddy-cli info drawing.dxf
cadaddy-cli convert --input folder/ --output out/ --format cdy
```
