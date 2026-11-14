# services.starah: levels of privilege for AI agents.
#
# Each agent `x` is a system user `starah-x` (group starah-agents). Agents are started
# sandboxed by starahd (`starah run --agent x -- CMD`). Every polkit question about an
# agent is answered by Starah's rule, which asks starahd; starahd answers from the
# agent's level and asks the human (Agent Request window) when the level says so.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.starah;
  agentUser = name: "starah-${name}";
  workspace = name: "/var/lib/starah-agents/${name}";

  configFile = pkgs.writeText "starah-config.json" (
    builtins.toJSON {
      agents = lib.mapAttrs (_: a: { inherit (a) level description; }) cfg.agents;
      classes = lib.recursiveUpdate (lib.importJSON cfg.classesFile) cfg.extraClasses;
      grant_minutes = cfg.grantMinutes;
      user_prefix = "starah-";
      workspaces = "/var/lib/starah-agents";
      polkit_user = "polkituser";
      agent_path = "/run/current-system/sw/bin";
    }
  );

  rules = ''
    // Starah: every authorization question about an agent account is answered by
    // starahd (through starah-polkit-check), from the agent's level. Runs before any
    // other rule, so nothing else can grant an agent more.
    polkit.addRule(function(action, subject) {
      if (!subject.isInGroup("starah-agents")) {
        return polkit.Result.NOT_HANDLED;
      }
      var detail = action.lookup("unit") || action.lookup("tulsi.summary") || "";
      try {
        var out = polkit.spawn(["${cfg.package}/bin/starah-polkit-check",
                                action.id, subject.user, detail]);
        return out.trim() == "yes" ? polkit.Result.YES : polkit.Result.NO;
      } catch (e) {
        polkit.log("starah: " + action.id + " for " + subject.user + ": " + e);
        return polkit.Result.NO;
      }
    });
  ''
  + lib.optionalString cfg.adminsWithoutPassword ''
    // Administrators at the machine answer agents' requests and start agents without
    // typing their password each time (changing levels still asks).
    polkit.addRule(function(action, subject) {
      if ((action.id == "org.starah.decide" || action.id == "org.starah.run") &&
          subject.local && subject.active && subject.isInGroup("wheel")) {
        return polkit.Result.YES;
      }
    });
  '';
in
{
  options.services.starah = {
    enable = lib.mkEnableOption "Starah, levels of privilege for AI agents";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "starahd, starah and starah-polkit-check.";
    };

    agents = lib.mkOption {
      default = { };
      description = ''
        The agents. Agent `x` runs as system user `starah-x`. `level` is the level until
        the human chooses one at first login (0 Observe, 1 Propose, 2 Routine,
        3 Administer, 4 Delegate).
      '';
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            level = lib.mkOption {
              type = lib.types.ints.between 0 4;
              default = 1;
              description = "Default level (1 = Propose: every privileged action is asked).";
            };
            description = lib.mkOption {
              type = lib.types.str;
              default = "";
              description = "Shown to the human next to the agent.";
            };
          };
        }
      );
    };

    classesFile = lib.mkOption {
      type = lib.types.path;
      default = ../data/classes.json;
      description = "polkit action id → class (observe, routine, admin, sensitive, forbidden).";
    };

    extraClasses = lib.mkOption {
      type = lib.types.attrs;
      default = { };
      example = {
        actions."org.example.frobnicate" = "routine";
      };
      description = "Merged over classesFile.";
    };

    grantMinutes = lib.mkOption {
      type = lib.types.ints.positive;
      default = 10;
      description = "How long an approved request stays granted by default.";
    };

    adminsWithoutPassword = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Let active local members of wheel answer requests and start agents without a
        password prompt. Changing levels always asks.
      '';
    };

    session = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Start starah-session (setup, login summary, requests) in graphical sessions.";
      };
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.callPackage ./session.nix { };
        defaultText = lib.literalExpression "pkgs.callPackage ./session.nix { }";
        description = "The starah-session package.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    security.polkit.enable = true;

    users.groups.starah-agents = { };
    users.users = lib.mapAttrs' (
      name: a:
      lib.nameValuePair (agentUser name) {
        isSystemUser = true;
        group = "starah-agents";
        home = workspace name;
        description = "Starah agent ${name}";
      }
    ) cfg.agents;

    systemd.tmpfiles.rules = [
      "d /var/lib/starah 0700 root root -"
      "d /var/lib/starah-agents 0755 root root -"
    ]
    ++ lib.mapAttrsToList (
      name: _: "d ${workspace name} 0700 ${agentUser name} starah-agents -"
    ) cfg.agents;

    environment.etc."starah/config.json".source = configFile;
    environment.etc."polkit-1/rules.d/00-starah.rules".text = rules;
    environment.systemPackages = [ cfg.package ] ++ lib.optional cfg.session.enable cfg.session.package;
    services.dbus.packages = [ cfg.package ];

    systemd.services.starahd = {
      description = "Starah privilege broker";
      wantedBy = [ "multi-user.target" ];
      after = [ "dbus.service" ];
      restartTriggers = [ configFile ];
      serviceConfig = {
        Type = "dbus";
        BusName = "org.starah.Broker1";
        ExecStart = "${cfg.package}/bin/starahd --config /etc/starah/config.json --state /var/lib/starah";
        Restart = "on-failure";
        ProtectSystem = "strict";
        ProtectHome = true;
        ReadWritePaths = [ "/var/lib/starah" ];
        PrivateTmp = true;
        NoNewPrivileges = true;
      };
    };

    systemd.user.services.starah-session = lib.mkIf cfg.session.enable {
      description = "Starah: agent privileges and requests";
      wantedBy = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      serviceConfig = {
        ExecStart = "${cfg.session.package}/bin/starah-session";
        Restart = "on-failure";
      };
    };

    assertions = [
      {
        assertion = lib.all (n: builtins.match "[a-z][a-z0-9-]*" n != null) (lib.attrNames cfg.agents);
        message = "services.starah.agents: names must be lowercase letters, digits and dashes.";
      }
    ];
  };
}
