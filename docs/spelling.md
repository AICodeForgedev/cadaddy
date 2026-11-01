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

## Where to get them

The lists are in the optional download repo **spell-dicts** (https://github.com/AICodeForgedev/spell-dicts),
kept outside CADaddy like craft-fonts; its README has a licence row per list. Get the `.txt` files into
the dictionary folder below.

| file | language | source | licence |
|---|---|---|---|
| `ens.txt`, `enz.txt` | British English (-ise / -ize) | SCOWL / English Speller Database (app.aspell.net) | SCOWL permissive notice (Kevin Atkinson) |
| `es.txt` | Spanish | LibreOffice es_ES dictionary | MPL-1.1 (of GPL-3.0+/LGPL-3.0+/MPL-1.1+) |
| `fr.txt` | French | Grammalecte French dictionaries | MPL-2.0 |
| `de.txt` | German | igerman98 | GPL-2.0 or GPL-3.0 |
| `pt.txt` | Portuguese (Portugal) | pt_PT, Universidade do Minho | MPL-1.1 (of GPL-2.0/LGPL-2.1/MPL-1.1) |
| `ptb.txt` | Portuguese (Brazil) | pt_BR VERO | MPL-2.0 (of LGPL-3.0/MPL-2.0) |

## Using your own Hunspell dictionary

A Hunspell `.dic` + `.aff` pair is not read directly; turn it into a plain list first:
`python3 tools/unmunch.py my_lang my_lang.words` (from spell-dicts; it reads `my_lang.dic` and
`my_lang.aff` and applies the prefix and suffix rules; with Hunspell installed, `unmunch my_lang.dic
my_lang.aff > my_lang.words` does the same). Put a short notice (source, licence), a line holding only
`---` and the words into `<code>.txt`.

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

## Testing the lists

`cargo test -p cadaddy-engine --test spell` also runs `the_downloadable_lists_check_their_languages`
when `CADADDY_SPELL_LISTS` names the folder holding the seven lists; without it the test says it is
skipped. For each language it checks that known words (one with accents) are not flagged and that a
misspelling's first suggestion is the right word.
