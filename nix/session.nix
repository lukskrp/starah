# starah-session: Starah's window in the desktop session (Python + PyGObject on OTK).
{
  lib,
  python3Packages,
  wrapGAppsHook3,
  gobject-introspection,
  otk,
  makeDesktopItem,
  copyDesktopItems,
}:

let
  root = ../apps/starah-session;
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
    copyDesktopItems
  ];
  buildInputs = [ otk ];
  dependencies = with python3Packages; [ pygobject3 ];

  nativeCheckInputs = [ python3Packages.pytestCheckHook ];
  pythonImportsCheck = [ "starah_session.model" ];

  desktopItems = [
    (makeDesktopItem {
      name = "org.starah.Session";
      desktopName = "Agent Privileges";
      comment = "Choose what AI agents may do on this computer";
      exec = "starah-session --show";
      icon = "org.starah.Session";
      categories = [
        "Settings"
        "System"
        "Security"
      ];
    })
  ];

  # The app icon: a balance with three bricks in each pan, in front of a Tulsi
  # leaf (data/make_icon.py generates it).
  postInstall = ''
    install -Dm644 data/org.starah.Session.svg \
      $out/share/icons/hicolor/scalable/apps/org.starah.Session.svg
  '';

  dontWrapGApps = true;
  preFixup = ''
    makeWrapperArgs+=("''${gappsWrapperArgs[@]}")
  '';

  meta = {
    description = "Starah in the desktop session: first-login setup, login summary and agent requests";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "starah-session";
  };
}
