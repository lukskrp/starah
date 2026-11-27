# Starah translations

Starah's window (Starah Setup, Agent Privileges, Compare Levels, Agent Request) and
its menu entry are translated with gettext. The catalogue's domain is `starah`, and
this directory holds it.

| File | What it is | Who edits it |
|---|---|---|
| `POTFILES` | The source files with translatable strings, relative to the repo root | Developers, when a file gains strings |
| `starah.pot` | The template: every English string, with notes for translators | Regenerated with `update-pot.sh`, never by hand |
| `update-pot.sh` | Regenerates `starah.pot` from `POTFILES` | — |
| `languages.txt` | The 78 languages to translate into: those TulsiOS's installer (Calamares) offers, in gettext codes | Developers |
| `<code>.po` | One language's translations, e.g. `de.po`, `pt_BR.po`, `sr@latin.po` | Translators |

The build (`nix/session.nix`) compiles each `<code>.po` into
`share/locale/<code>/LC_MESSAGES/starah.mo`. It also writes `Name[xx]=`,
`Comment[xx]=` and `Keywords[xx]=` into the menu entry. A language appears as
soon as its `.po` file exists. A missing or empty translation shows English, so
a partial file is safe.

Starah follows the person's desktop language (`LANGUAGE`, `LC_ALL`,
`LC_MESSAGES`, `LANG`).

TulsiOS has its own catalogue for its own apps and launchers (domain `tulsios`,
`po/` in the TulsiOS repository, same languages). If you are translating both,
do them in the same session, with the same terms.

---

## Brief for the translating agent

You are translating the Starah window into every language in `languages.txt`.
Work in a checkout of the Starah repository.

### 1. Set up

You need gettext: `nix shell nixpkgs#gettext`, or `nix develop` in TulsiOS.

```sh
bash po/update-pot.sh        # make sure starah.pot is current
grep -v '^#' po/languages.txt | while read -r code; do
  [ -e "po/$code.po" ] || msginit --no-translator -l "$code" -i po/starah.pot -o "po/$code.po"
done
```

In each new file, check the header:
- `"Language: <code>\n"`;
- `"Content-Type: text/plain; charset=UTF-8\n"`;
- the language's usual `Plural-Forms`. msginit fills this in for most languages.
  Starah uses no plurals today, but leave it correct.

### 2. Translate

Fill in every `msgstr`. Never change a `msgid`. Read the `#.` comments above an
entry: they say where the string shows and what to keep.

**Rules for every language:**
- **Placeholders:** keep every `{name}` placeholder exactly as written: `{agent}`,
  `{level}`, `{what}`, `{action}`, `{kind}`, `{detail}`, `{reason}`, `{error}`,
  `{line}`, `{message}`, `{allow}`, `{ask}`, `{deny}`, `{verdict}`. Move them
  wherever the sentence needs them, but don't translate, drop or add any. A wrong
  placeholder crashes the window.
- **Access keys:** keep the `_` in `_Save`, `_Close`, `_Approve`, `_Reject` and
  `_Save and continue`. Put it before a letter of your translation, preferably the
  first, and use different letters for buttons in the same window (Save and Close
  are together; so are Approve and Reject).
- **Never translate:**
  - Starah;
  - NixOS, TulsiOS, systemd, pkexec;
  - Wi-Fi, USB, AppImage, `.deb`, `.rpm`, SSH, git, npm, pip;
  - file paths (`/etc/starah/rules.rhai`);
  - the marks ✓ ? ✗.
- **Things that must match:**
  - "Agent Privileges" and "Edit Rules…" are the names of the window and a button.
    Wherever another string mentions them, use exactly the same translation.
  - "Agent" means an AI agent: a program that acts on its own. Use the term your
    language's software uses for AI agents, if there is one.
- **The five level names:** Observe, Propose, Routine, Administer, Delegate. They
  show in a menu and as table headings, so keep them short, one word if possible.
  Make them distinct from each other, and in the same form (all verbs, or all
  nouns).
- **Table cells:** "On its own", "Asks you first" and "Not allowed" sit under ✓,
  ? and ✗ in narrow columns. Aim for about 20 characters at most.
- **Examples in the table:** the bullet points are lowercase fragments with no
  final period. Keep that style if it is natural in the language.
- **Addressing the person:** use the form your language's desktop software uses
  (GNOME, Cinnamon, KDE), for example "du" in Swedish and "Sie" in German, and
  keep it the same throughout.
- **Typography:** use your language's quotation marks for “Agent Privileges” and
  “{agent}”, and its punctuation and spacing (French puts a space before `:`).
- **Keywords:** the menu entry's keywords
  (`AI;agent;privileges;permissions;Starah;security;`) keep the `;` between words
  and at the end. Translate the words, keep Starah, and add local synonyms if
  people would search for them.
- **When unsure, leave it out:** if you are not confident in a language, leave
  that string's `msgstr ""` empty. It shows in English, which is better than a
  wrong translation.

**Languages that need care:**
- **Right to left:** `ar`, `fa`, `he`, `ur`. Write normally; GTK lays the window
  out. Keep placeholders and ✓ ? ✗ as they are.
- **Variants:** write each variant as people there write it:
  - `pt` and `pt_BR`;
  - `es` and `es_AR`;
  - `zh_CN` (simplified) and `zh_TW`, `zh_HK` (traditional);
  - `sr` (Cyrillic) and `sr@latin` (Latin script);
  - `ca@valencia`.
  Don't copy one variant into the other.
- **Fallbacks:** `ie` and `eo` are constructed languages. Translate them; if you
  can't, leave the strings empty.

### 3. Check

```sh
for f in po/*.po; do msgfmt -c --check-format -o /dev/null "$f" || echo "BROKEN: $f"; done
git add po/*.po                     # flakes only see files git knows about
nix build .#checks.x86_64-linux.translations   # the template is current; every .po compiles
nix build .#checks.x86_64-linux.starah-session # the package, its tests and the .mo files
grep -c '^Name\[' result/share/applications/org.starah.Session.desktop   # one per language
```

`msgfmt --check-format` doesn't understand Python's `{name}` placeholders, so also
make sure every placeholder of a `msgid` is in its `msgstr`. This prints any that
aren't:

```sh
python3 - <<'PY'
import re, glob, subprocess
for f in sorted(glob.glob("po/*.po")):
    out = subprocess.run(["msgattrib", "--translated", "--no-wrap", f], capture_output=True, text=True).stdout
    for mid, mstr in re.findall(r'^msgid "(.*)"\nmsgstr "(.*)"$', out, re.M):
        if mid and set(re.findall(r"\{\w+\}", mid)) != set(re.findall(r"\{\w+\}", mstr)):
            print(f, "|", mid[:60])
PY
```

To see one language on screen, on a system running the built package:

```sh
LANGUAGE=de starah-session --compare
```

The table shows every level, every row and every cell.

### 4. Later: when the English changes

```sh
bash po/update-pot.sh
for f in po/*.po; do msgmerge -U --backup=none "$f" po/starah.pot; done
```

Then translate the new entries and the ones marked `#, fuzzy`, remove the `fuzzy`
marks, and check again.

---

## For developers

- **Mark new strings:** wrap them in `_()` from `starah_session.i18n`, then run
  `po/update-pot.sh` and commit `starah.pot`. `checks.translations` fails until
  you do.
- **Placeholders:** use `_("… {name} …").format(name=…)`, never f-strings, which
  gettext can't extract.
- **Notes:** a `# Translators:` comment just above the call reaches the
  translator.
- **New files:** add a new source file to `POTFILES`.
