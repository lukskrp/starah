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
      };
    };
}
