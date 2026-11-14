# starahd (the broker), starah (CLI) and starah-polkit-check, plus the D-Bus and
# polkit files they need.
{ lib, rustPlatform }:

let
  root = ../.;
in
rustPlatform.buildRustPackage {
  pname = "starah";
  version = "0.1.0";

  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (root + "/Cargo.toml")
      (root + "/Cargo.lock")
      (root + "/crates")
      (root + "/data")
    ];
  };

  cargoLock.lockFile = root + "/Cargo.lock";

  postInstall = ''
    install -Dm444 data/dbus/org.starah.Broker1.conf -t $out/share/dbus-1/system.d
    install -Dm444 data/polkit/org.starah.policy -t $out/share/polkit-1/actions
    install -Dm444 data/classes.json -t $out/share/starah
  '';

  meta = {
    description = "Starah: levels of privilege for AI agents (broker, CLI, polkit helper)";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "starah";
  };
}
