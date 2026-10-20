# AutoLISP

CADaddy runs AutoLISP routines (`.lsp` files). What is here and what is not yet:
[gaps.md](gaps.md) (E1).

## Loading and running

- **APPLOAD** (Tools > Load Application…, alias `AP`) asks for a `.lsp` file and runs it. Dropping
  a `.lsp` file on the window does the same.
- **`(load "name")`** loads a file by path, with `.lsp` added when the name has none. A file
  APPLOAD loaded is found again by its name, also on the web where there is no file system.
- **`(defun c:NAME …)`** defines a command: type `NAME` at the command line. Built-in commands
  keep their names.
- **At the command line**, `(expression)` evaluates and shows the value, and `!variable` shows a
  variable. Typed at a command's prompt (not a free-text one), an expression's value answers it:
  `CIRCLE 0,0 (* 2 r)`.
- **Programmatically** (scripts, the control channel, MCP, the CLI): `lisp {code, inputs?,
  dialog?}` and `appload {path} | {name, text | data}` run to the end and return `{value, output,
  error?}`. `inputs` answer `get…` calls in order (numbers, strings, `[x, y]` points); a call with
  none left gets nil. `dialog` answers `start_dialog`: tile actions in order, `{key, value?,
  reason?}`; a dialog still open when they run out is cancelled (status 0).

### At startup

CADaddy loads LISP from its **support folder**, `support` in the settings folder (on Windows
`%APPDATA%\CADaddy\support`, on macOS `~/Library/Application Support/CADaddy/support`, on
Linux `~/.config/cadaddy/support`; created at first start):

- `acad.lsp` once per session, into the first drawing; with `ACADLSPASDOC` set to 1, into every
  drawing;
- `acaddoc.lsp` into every drawing opened or created;
- then `S::STARTUP`, when they define it.

`(autoload "file" '("CMD1" "CMD2"))` in a startup file makes `CMD1` and `CMD2` commands that load
`file` (from the support folder too) the first time they are typed. Startup files run to the end
without asking for input. A drawing's own folder is never searched: an `acaddoc.lsp` that came
with a downloaded drawing does not run.

Each drawing has its own variables and functions. A routine is one undo step, whatever commands
it runs. A routine that waits for input (`getpoint`, `pause`) lets the window work meanwhile;
Esc stops it with "Function cancelled", which `*error*` receives. A long loop runs a slice per
frame, so the window stays live and Esc stops it too.

### Dialogs (DCL)

`load_dialog` reads a `.dcl` file (by path, from the support folder, or one APPLOAD loaded — on
the web too), `new_dialog` makes one of its dialogs and `start_dialog` shows it. Each click or
edit runs the tile's action — `action_tile`, the tile's `action` attribute, else OK ends with 1
and Cancel with 0 — with `$key`, `$value`, `$reason` and `$data` set, until an action calls
`done_dialog` (or `term_dialog`, -1). Tiles: `dialog row column boxed_row boxed_column
radio_row radio_column boxed_radio_row boxed_radio_column concatenation paragraph button
edit_box toggle radio_button popup_list list_box slider text text_part image image_button
spacer errtile`, the standard clusters (`ok_only ok_cancel ok_cancel_help
ok_cancel_help_errtile …`), definitions built on others, and `@include`. Images show
`fill_image` and `vector_image`; `slide_image` shows the slide's name (slide files are not
read). Dialogs nest (an action may start another).

An agent drives a dialog a typed routine waits in with `lisp.dialog` (its tiles and values) and
`lisp.tile {key, value?} | {cancel: true}`.

### Reactors

`vlr-command-reactor` and `vlr-editor-reactor` call their functions on `:vlr-commandWillStart`,
`:vlr-commandEnded`, `:vlr-commandCancelled` and `:vlr-commandFailed` with `(reactor
("COMMAND"))`; `vlr-object-reactor` on `:vlr-modified` and `:vlr-erased` of its objects with
`(object reactor nil)`. Also `vlr-remove vlr-add vlr-added-p vlr-remove-all vlr-reactors
vlr-data vlr-data-set vlr-type vlr-reactions vlr-owners`. Reactors live in their drawing until
removed. They react to commands typed or called by an agent, not to those a routine runs, and
`command` inside a callback is refused.

## The language

Integers are 32-bit and wrap as AutoLISP's do; any real makes arithmetic real. Variables are
dynamically scoped: a function's arguments and its locals (after `/`) are bound while it runs.
Symbols are case-insensitive. Reals print with six significant digits.

Nothing a routine does can crash the program: deep recursion is the error "internal stack limit
reached", malformed source is a reader error, and every built-in checks its arguments.

## Built-in functions

- **Forms:** `quote` `'` `function` `setq` `set` `if` `cond` `while` `repeat` `foreach` `progn`
  `and` `or` `defun` `defun-q` `lambda` `eval` `apply` `mapcar` `load` `autoload` `exit` `quit`
- **Numbers:** `+ - * / rem 1+ 1- abs min max sqrt exp log sin cos atan expt fix float gcd
  logand logior lsh ~ = /= < > <= >= zerop minusp numberp`
- **Lists:** `car cdr` and the `c…r` combinations up to four levels, `cons list append reverse
  length nth last member assoc subst acad_strlsort`, `vl-position vl-remove vl-sort
  vl-remove-if vl-remove-if-not vl-member-if vl-every vl-some vl-list-length vl-consp`
- **Predicates:** `atom listp null not type eq equal boundp vl-symbolp`
- **Strings:** `strcat strlen substr strcase itoa atoi atof chr ascii read wcmatch`,
  `vl-string-search vl-string-subst vl-string-trim vl-string-left-trim vl-string-right-trim
  vl-string-translate vl-string->list vl-list->string vl-prin1-to-string vl-princ-to-string
  vl-symbol-name vl-symbol-value vl-filename-base vl-filename-extension vl-filename-directory`
- **Geometry:** `polar distance angle inters trans` (`trans` returns the point: no UCS yet)
- **Output and files:** `princ prin1 print prompt terpri alert open close read-line write-line
  findfile`
- **Errors:** `*error*`, `vl-catch-all-apply vl-catch-all-error-p vl-catch-all-error-message`
- **Drawing:** `command` (also `command-s`, `vl-cmdf`) with `pause`, a command left open for the
  next call, and `(command)` to cancel; `getvar setvar`; `entget entmake entmod entdel entlast
  entnext entupd handent`; `ssget` (modes X, L, P, I, W, C, F, WP, CP, a point, or the user
  selecting; filters with wildcards and `-4` operators) `ssadd ssdel sslength ssname ssmemb
  sssetfirst ssgetfirst`; `tblsearch tblnext` (LAYER, LTYPE, STYLE, BLOCK, DIMSTYLE, APPID)
- **Input:** `initget getpoint getcorner getdist getangle getorient getreal getint getstring
  getkword entsel nentsel`
- **Units:** `rtos angtos distof angtof`
- **No-ops:** `vl-load-com gc redraw`; `ver`

Entity lists use the DXF group codes of CADaddy's DXF files: what `entget` returns is what a
DXF file holds for the object, plus `-1` (its name) and `410` (its layout).

## Testing against the corpus

The AutoLISP conformance corpus (#648) lives in its own repository. Point `CADADDY_LISP_CORPUS`
at it and run `cargo test -p cadaddy-engine --test lisp_corpus`; without the variable the test
says it was skipped.
