"""Translations: gettext domain "starah" (catalogues in the repo's po/).

The package's wrapper sets STARAH_LOCALEDIR to its share/locale; without it (tests,
a checkout) the system's locale directories are searched, and every string falls
back to English when there is no catalogue for the person's language.
"""

from __future__ import annotations

import gettext
import os

DOMAIN = "starah"

_translation = gettext.translation(
    DOMAIN, localedir=os.environ.get("STARAH_LOCALEDIR") or None, fallback=True
)
_ = _translation.gettext
ngettext = _translation.ngettext
