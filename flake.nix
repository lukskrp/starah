{
  description = "Starah — levels of privilege for AI agents on Linux (polkit + systemd + a broker)";

  inputs = {
    # tulsipkgs (TulsiOS's nixpkgs) provides OTK for starah-session.
    nixpkgs.url = "github:lukskrp/tulsipkgs/tulsi-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      overlays.default = final: _prev: {
        starah = final.callPackage ./nix/package.nix { };
        starah-session = final.callPackage ./nix/session.nix { };
      };

      nixosModules.default = ./nix/module.nix;

      packages = forAllSystems (pkgs: rec {
        starah = pkgs.callPackage ./nix/package.nix { };
        starah-session = pkgs.callPackage ./nix/session.nix { };
        default = starah;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
          ];
        };
      });

      checks.x86_64-linux = {
        inherit (self.packages.x86_64-linux) starah starah-session;
        agents = nixpkgs.legacyPackages.x86_64-linux.testers.runNixOSTest (
          import ./nix/tests/agents.nix { inherit self; }
        );
        # po/starah.pot has every string the window marks for translation, and
        # each translation compiles (po/README.md).
        translations =
          let
            pkgs = nixpkgs.legacyPackages.x86_64-linux;
          in
          pkgs.runCommand "starah-translations" { nativeBuildInputs = [ pkgs.gettext ]; } ''
            cp -r ${self}/. src && chmod -R u+w src && cd src
            cp po/starah.pot committed.pot
            bash po/update-pot.sh
            if ! diff -u <(grep -v '^#:' committed.pot) <(grep -v '^#:' po/starah.pot); then
              echo "po/starah.pot is out of date: run po/update-pot.sh and commit it" >&2
              exit 1
            fi
            for f in po/*.po; do
              [ -e "$f" ] || continue
              msgfmt -c -o /dev/null "$f" || { echo "broken: $f" >&2; exit 1; }
            done
            touch $out
          '';
      };
    };
}
