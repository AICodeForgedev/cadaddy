# CADaddy and AI agents (MCP)

CADaddy speaks the [Model Context Protocol](https://modelcontextprotocol.io) over stdio.

## Start it

```sh
# The usual way (what the Claude connector runs): drives the open CADaddy window when one is
# listening on the agent port, 127.0.0.1:47820, else runs a drawing session of its own.
cadaddy-cli mcp

# Always a session of its own, never the window:
cadaddy-cli mcp --headless

# A specific app's control port (an app started with `cadaddy --control 7979`):
cadaddy-cli mcp --connect 127.0.0.1:7979
```

The desktop app listens on the agent port **47820** (this computer only) unless "Let AI agents control
CADaddy" is turned off in Options ▸ User Preferences; a `--control PORT` given on its command line is
served as well, not instead. Without `--connect`, `cadaddy-cli mcp` decides the first time it is asked
(listing its tools or its first call): the window if one answers on 47820, else its own session — so
open CADaddy before asking the agent if you want to watch it work.

With `--connect`, an MCP client that starts before the app can add `--lazy` (reach the app at the
first tool call; until it answers each call fails with a message saying how to open it) and
`--launch` (open CADaddy with that control port when nothing answers, and wait for it — macOS, and on
Windows and Linux the app installed beside `cadaddy-cli`). `--connect` needs the app's `HOST:PORT`; a
missing or bad one is an error, never a silent session of its own.

Example client configuration (Claude Code / Claude Desktop):

```json
{ "mcpServers": { "cadaddy": { "command": "cadaddy-cli", "args": ["mcp"] } } }
```

## Tools

| Tool | What it does |
|---|---|
| `command_line {text}` | Type at the command line exactly like a person: `line 0,0 @10,0 @0,5 c`, `circle 5,2 1`, `offset 0.5`, `zoom e`. Spaces act as Enter; empty text is Enter. Returns the new prompt, keywords and output. |
| `script {text}` | A multi-line script (like `.scr` files). |
| `execute {command, params}` | Any command with JSON parameters, never a dialog. `list_commands` shows ids and parameter docs. |
| `list_commands {filter?}` | The command catalog; `filter` matches part of a command's id, label or an alias. |
| `inspect_drawing {entities?, limit?}` | Counts, extents, layers, styles, blocks, layouts, selection, undo history and entities. |
| `query_entities {type?, layer?, window?, crossing?, limit?, offset?}` | Filtered entities with handles and geometry. `window` `[[x0,y0],[x1,y1]]` finds objects whose geometry is inside it or crosses it (`crossing: false`: entirely inside). `count` is every match, `returned` the page (`offset`, `limit`, default 500). |
| `render {width?, height?, fit?, window?}` | A PNG image of the drawing, or of just the area `window` `[[x0,y0],[x1,y1]]` (one sheet or room of a large drawing; 1600 px wide with the window's shape unless sized). |
| `find_text {contains?, layer?, window?, limit?, offset?}` | The readable texts — TEXT, MTEXT without formatting codes, block attributes, multileaders — with text, insertion point, height, rotation, style, layer and handle; filtered by content (any case), layer and a window around the insertion points. Read-only (the `texts` command). |
| `new_drawing {metric?}`, `open {path}`, `save {path?}`, `cancel` | Files and Escape. `save` to a `.svg`, `.png` or `.pdf` path exports: the drawing keeps its name and unsaved changes. |
| `screenshot`, `ui_inspect`, `ui_click {x, y}` | Only when connected to the app. |
| `list_drawings {}` / `switch_drawing {index\|path\|title}` | The open drawings (title, path, unsaved changes, current) and switching between them. |
| `close_drawing {index?, path?, title?, save?, saveAs?, discard?}` / `close_all {save?, discard?}` | Close drawings; unsaved work is never dropped unless `discard` is given (or it's saved first). |
| `undo {count?}` / `redo {count?}` | Undo or redo commands. |
| `select {handles\|window\|all\|clear, crossing?, add?}` | Set the selection; returns the selected handles. |
| `get_variable {name}` / `set_variable {name, value}` | System variables, checked like SETVAR (unknown names are refused). |
| `run_lisp {code, inputs?}` | Evaluate AutoLISP; an evaluation error is a tool error. |
| `plot {path, layout?}` | Plot a layout (or Model) to PDF with its page setup. |
| `get_prompt {}` | What CADaddy is asking for: command, prompt, keywords, default, accepted input, `busy`, `closePrompt`. |
| `wait_until_idle {timeoutMs?}` | Wait while a drawing is still opening in the background. |
| `answer_close_prompt {choice}` / `quit_app {save?, discard?}` | (Window only) answer "Save changes to …?" as the user would; quit without losing unsaved work. |

Resources: `cadaddy://drawing` (inspect JSON) and `cadaddy://commands` (catalog).

Arguments are checked against each tool's input schema. An unknown tool, `arguments` that aren't an
object, a missing required argument or one of the wrong type is a JSON-RPC error `-32602` (invalid
params), so `command_line {}` never presses Enter by accident. A tool that runs and fails (a bad
point, an unknown command) returns a result with `isError: true` and the message.

## A typical session

1. `command_line {"text": "rectang 0,0 100,60"}`
2. `execute {"command": "circle", "params": {"center": [10, 10], "radius": 4}}`
3. `command_line {"text": "dimlinear 0,0 100,0 50,-8"}`
4. `inspect_drawing {}` — verify counts and geometry.
5. `render {}` — look at it.

The end-to-end test in `crates/mcp/src/tests.rs` runs a session like this and checks the result
only through MCP.
