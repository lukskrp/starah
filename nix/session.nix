# starah-session: Starah's window in the desktop session (Python + PyGObject on OTK).
{
  lib,
  python3Packages,
  wrapGAppsHook3,
  gobject-introspection,
  otk,
  gettext,
}:

let
  root = ../apps/starah-session;
  # Translations (gettext domain "starah"): po/<code>.po, see po/README.md.
  po = lib.fileset.toSource {
    root = ../po;
    fileset = ../po;
  };
in
python3Packages.buildPythonApplication {
  pname = "starah-session";
  version = "0.1.0";
  pyproject = true;

  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.difference root (lib.fileset.fileFilter (f: f.hasExt "pyc") root);
  };

  build-system = [ python3Packages.setuptools ];

  nativeBuildInputs = [
    wrapGAppsHook3
    gobject-introspection
    gettext
  ];
  buildInputs = [ otk ];
  dependencies = with python3Packages; [ pygobject3 ];

  nativeCheckInputs = [
    python3Packages.pytestCheckHook
    gettext
  ];
  pythonImportsCheck = [ "starah_session.model" ];

  # The app icon: a balance with three bricks in each pan, in front of a Tulsi
  # leaf (data/make_icon.py generates it). Then the translations: one .mo per
  # po/<code>.po, and the menu entry with Name[xx]= etc. from the same files
  # (msgfmt --desktop reads the languages from LINGUAS: the .po files there are).
  postInstall = ''
    install -Dm644 data/org.starah.Session.svg \
      $out/share/icons/hicolor/scalable/apps/org.starah.Session.svg

    mkdir -p po && cp -r ${po}/. po/ && chmod -R u+w po
    find po -maxdepth 1 -name '*.po' -printf '%f\n' | sed 's/\.po$//' | sort > po/LINGUAS
    while read -r lang; do
      install -d $out/share/locale/$lang/LC_MESSAGES
      msgfmt -c -o $out/share/locale/$lang/LC_MESSAGES/starah.mo po/$lang.po
    done < po/LINGUAS
    if [ -s po/LINGUAS ]; then
      msgfmt --desktop --template=data/org.starah.Session.desktop.in -d po \
        -o org.starah.Session.desktop
    else
      cp data/org.starah.Session.desktop.in org.starah.Session.desktop
    fi
    install -Dm644 org.starah.Session.desktop -t $out/share/applications
  '';

  dontWrapGApps = true;
  preFixup = ''
    makeWrapperArgs+=(
      "''${gappsWrapperArgs[@]}"
      --set-default STARAH_LOCALEDIR $out/share/locale
    )
  '';

  meta = {
    description = "Starah in the desktop session: first-login setup, login summary and agent requests";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "starah-session";
  };
}
