#!/usr/bin/env bash
# Regenerates po/starah.pot from the files in po/POTFILES (run from anywhere;
# needs gettext: `nix shell nixpkgs#gettext`). Then update each language with
# `msgmerge -U po/<code>.po po/starah.pot`.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
grep '\.py$' po/POTFILES > "$tmp/py"
grep '\.desktop\.in$' po/POTFILES > "$tmp/desktop"
common=(--from-code=UTF-8 --package-name=Starah --msgid-bugs-address=https://github.com/lukskrp/starah)
xgettext "${common[@]}" --language=Python --keyword=_ --keyword=ngettext:1,2 \
  --add-comments=Translators -f "$tmp/py" -o "$tmp/py.pot"
xgettext "${common[@]}" --language=Desktop -k -kName -kComment -kKeywords \
  -f "$tmp/desktop" -o "$tmp/desktop.pot"
msgcat --use-first "$tmp/py.pot" "$tmp/desktop.pot" -o po/starah.pot
# No creation date: it would make every regeneration a change (and the check
# that the template is current compare unequal).
sed -i '/^"POT-Creation-Date: /d' po/starah.pot
echo "po/starah.pot: $(grep -c '^msgid "' po/starah.pot) messages (including the header)"
