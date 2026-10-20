# Spelling dictionaries

SPELL (Tools ▸ Spelling) checks text against a main dictionary and the user dictionary
(DCTCUST). American English ships with CADaddy (`assets/spell/en_US.txt`). Other languages are
optional downloads, kept outside this repository (like the fonts): each is a word list with its
own licence.

## Choosing one

The main dictionary belongs to the drawing: DCTMAIN (SETVAR, or the Check Spelling dialog's
"Main dictionary" list) holds one of AutoCAD's codes:

| code | language |
|---|---|
| `enu` | American English (ships) |
| `ens` | British English (-ise) |
| `enz` | British English (-ize) |
| `es` | Spanish |
| `fr` | French |
| `de` | German |
| `pt` | Portuguese (Iberian) |
| `ptb` | Portuguese (Brazilian) |

The choice is saved with the drawing (.cdy, and in DXF and DWG as CADaddy's `CADADDY_DCTMAIN`
record). A drawing that names none uses American English.

## Installing one

Put `<code>.txt` (for example `fr.txt`) in the dictionary folder:

- `CADADDY_SPELL_DIR` when set, else
- `dictionaries` in CADaddy's settings folder (`CADADDY_CONFIG_DIR`, or
  `~/Library/Application Support/CADaddy` on macOS, `%APPDATA%\CADaddy` on Windows,
  `~/.config/cadaddy` elsewhere).

The file is UTF-8 text, one word per line, as it is spelled (accents kept). Lines starting with
`#` are comments; anything before a line holding only `---` is the list's notice (its source and
licence) and is not read as words. Files over 64 MB are not read. A list is read the first time
a check needs it.

When the drawing's dictionary isn't installed, SPELL says which file to put where; the dialog
marks languages not installed.

The SPELL command's `{"action": "dictionaries"}` lists them (code, language, installed), with the
drawing's choice and the folder.
