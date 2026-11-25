# Starah end to end with real systemd and polkit: agents run as their own sandboxed
# users, polkit asks starahd about them, and the levels decide.
{ self }:
{
  name = "starah-agents";

  nodes.machine =
    { modulesPath, pkgs, ... }:
    {
      imports = [
        self.nixosModules.default
        (modulesPath + "/../tests/common/user-account.nix")
      ];
      users.users.alice.extraGroups = [ "wheel" ];

      services.starah = {
        enable = true;
        session.enable = false;
        agents.tester = {
          level = 0;
          description = "Test agent";
        };
        agents.helper.description = "Asks before acting";
        # Made in the agent's own home at each interactive session.
        agents.tester.homeLinks.".config/test/linked" = "/etc/hostname";
      };
      # `su` shells are not active local sessions: let wheel steer Starah in the test.
      security.polkit.extraConfig = ''
        polkit.addRule(function(action, subject) {
          if (action.id.indexOf("org.starah.") == 0 && subject.isInGroup("wheel")) {
            return polkit.Result.YES;
          }
        });
      '';

      systemd.services.demo.script = "sleep infinity";
      systemd.services.demo2.script = "sleep infinity";
      environment.systemPackages = [ pkgs.jq ];
      # An interactive agent session, as a person starts one in their terminal
      # (`script` gives the client a real terminal). Prints what the agent sees.
      environment.etc."starah-test/session".source = pkgs.writeShellScript "session" ''
        cd "$1"
        exec script -qec "stty rows 33 cols 101; starah run --interactive --agent tester -- /run/current-system/sw/bin/sh -c 'id -un; pwd; echo HOME=\$HOME; stty size; echo made > made.txt; ls -A /home/alice; readlink \$HOME/.config/test/linked; echo ARGS:\$#' " /dev/null
      '';
      # Asks polkit whether the calling process may perform an action.
      environment.etc."starah-test/pk".source = pkgs.writeShellScript "pk" ''
        exec ${pkgs.polkit.bin}/bin/pkcheck --action-id "$1" --process $$
      '';
    };

  testScript = ''
      import json

      def alice(cmd):
          return f"su - alice -c '{cmd}'"

      def agent(name, cmd):
          return alice(f"starah run --agent {name} -- {cmd}")

      def requests():
          out = machine.succeed(alice("starah requests")).strip()
          return [line.split("\t") for line in out.splitlines() if line]

      machine.wait_for_unit("starahd.service")
      machine.wait_for_unit("multi-user.target")

      with subtest("an agent is its own user, in a sandbox it cannot leave"):
          assert machine.succeed(agent("tester", "id -un")).strip() == "starah-tester"
          machine.succeed(agent("tester", "touch /var/lib/starah-agents/tester/ok"))
          machine.fail(agent("tester", "touch /etc/x"))
          machine.fail(agent("tester", "ls /home/alice"))
          machine.fail(agent("tester", "cat /var/lib/starah/policy.json"))
          out = machine.fail(agent("tester", "/run/wrappers/bin/sudo -n true 2>&1"))
          machine.log(f"sudo inside the agent: {out}")
          machine.fail(agent("tester", "/run/wrappers/bin/su -c true alice </dev/null"))
          # Exit codes come back through starah run.
          machine.succeed(agent("tester", "true"))
          assert machine.execute(agent("tester", "sh -c \"exit 7\""))[0] == 7

      with subtest("level 0 (Observe): privileged actions are denied, nothing is asked"):
          machine.fail(agent("tester", "systemctl start demo.service"))
          machine.fail("systemctl is-active demo.service")
          assert requests() == [], requests()

      with subtest("only humans steer Starah"):
          machine.fail(agent("tester", "starah set tester 4"))
          machine.fail(agent("tester", "starah requests"))
          machine.fail(agent("tester", "starah audit"))
          machine.fail(alice("busctl call org.starah.Broker1 /org/starah/Broker1 org.starah.Broker1 "
                             "Check sss org.tulsi.apply starah-tester \"\""))
          policy = json.loads(machine.succeed("busctl --json=short call org.starah.Broker1 "
                                              "/org/starah/Broker1 org.starah.Broker1 GetPolicy "
                                              "| jq -r .data[0]"))
          assert not policy["configured"], policy

      with subtest("the human chooses levels"):
          machine.succeed(alice("starah set tester 1 helper 1"))
          status = machine.succeed(alice("starah status"))
          assert "Configured: yes" in status and "level 1 (Propose)" in status, status
          machine.succeed("grep -q '\"configured\": true' /var/lib/starah/policy.json")

      with subtest("level 1 (Propose): denied, asked, approved for that unit, then allowed"):
          machine.fail(agent("tester", "systemctl start demo.service"))
          reqs = requests()
          # systemd may ask polkit for more than one permission per refused call.
          assert reqs and all(r[1] == "tester" and "(demo.service)" in r[3] for r in reqs), reqs
          assert any("manage-units" in r[3] for r in reqs), reqs
          # Retrying before the human answers does not pile up requests.
          machine.fail(agent("tester", "systemctl start demo.service"))
          assert len(requests()) == len(reqs), requests()
          for r in reqs:
              machine.succeed(alice(f"starah approve {r[0]} 5"))
          machine.succeed(agent("tester", "systemctl start demo.service"))
          machine.succeed("systemctl is-active demo.service")
          machine.succeed(agent("tester", "systemctl stop demo.service"))
          # The approval covers demo.service only.
          machine.fail(agent("tester", "systemctl start demo2.service"))
          reqs = requests()
          assert reqs and all("(demo2.service)" in r[3] for r in reqs), reqs
          for r in reqs:
              machine.succeed(alice(f"starah reject {r[0]}"))
          machine.fail(agent("tester", "systemctl start demo2.service"))
          for r in requests():  # the retry asked again; answer no
              machine.succeed(alice(f"starah reject {r[0]}"))

      def ask_in_background(tag, title, action="org.tulsi.apply"):
          machine.succeed(alice(
              f"(starah run --agent helper -- starah ask {action} \"{title}\" \"+ package cowsay\"; "
              f"echo $? > /tmp/{tag}.rc) > /tmp/{tag}.log 2>&1 &"
          ))
          machine.wait_until_succeeds(alice("starah requests") + f" | grep -q \"{title}\"")
          return next(r for r in requests() if r[3] == title)

      with subtest("a cooperating agent asks first and waits for the human"):
          req = ask_in_background("ask1", "install cowsay")
          assert req[1] == "helper" and req[2] == "routine", req
          machine.sleep(1)
          machine.fail("test -e /tmp/ask1.rc")  # still waiting
          machine.succeed(alice(f"starah approve {req[0]}"))
          machine.wait_until_succeeds("grep -qx 0 /tmp/ask1.rc")
          req = ask_in_background("ask2", "install cowsay again")
          machine.succeed(alice(f"starah reject {req[0]}"))
          machine.wait_until_succeeds("grep -qx 1 /tmp/ask2.rc")

      with subtest("level 2 (Routine): what a rollback undoes goes ahead, the rest asks"):
          machine.succeed(alice("starah set helper 2"))
          machine.succeed(
              "starah run --agent helper -- starah ask org.tulsi.apply 'install hello' '+ package hello'"
          )
          assert requests() == [], requests()
          req = ask_in_background("ask3", "add user bob", "org.tulsi.configure")
          assert req[2] == "admin", req
          machine.succeed(alice(f"starah reject {req[0]}"))
          machine.wait_until_succeeds("grep -qx 1 /tmp/ask3.rc")

      with subtest("level 3 (Administer): administration without asking"):
          machine.succeed(alice("starah set tester 3"))
          machine.succeed(agent("tester", "systemctl start demo2.service"))
          machine.succeed("systemctl is-active demo2.service")
          assert requests() == [], requests()
          # Sensitive actions are still asked.
          machine.fail(agent("tester", "/etc/starah-test/pk org.freedesktop.systemd1.manage-unit-files"))
          assert len(requests()) == 1, requests()

      with subtest("level 4 (Delegate): everything but the forbidden class"):
          machine.succeed(alice("starah set tester 4"))
          machine.succeed(agent("tester", "/etc/starah-test/pk org.freedesktop.systemd1.manage-unit-files"))
          machine.fail(agent("tester", "/etc/starah-test/pk org.freedesktop.policykit.exec"))
          machine.fail(agent("tester", "/etc/starah-test/pk org.starah.configure"))

      machine.succeed("systemctl stop demo2.service")

      with subtest("every decision is audited, and the levels survive a restart"):
          audit = machine.succeed(alice("starah audit 500"))
          for needle in ['"event":"set-policy"', '"event":"decide"', '"approved":true',
                         '"event":"check"', '"event":"ask"', '"event":"run"']:
              assert needle in audit, needle
          machine.succeed("systemctl restart starahd.service")
          machine.wait_for_unit("starahd.service")
          assert "level 4 (Delegate)" in machine.succeed(alice("starah status"))

      with subtest("interactive: the agent's own user, on a terminal, in the project folder only"):
          machine.succeed(alice("mkdir -p ~/proj ~/secret && echo s > ~/secret/s && echo ~ > /dev/null"))
          out = machine.succeed(alice("/etc/starah-test/session /home/alice/proj"))
          machine.log(out)
          assert "starah-tester" in out and "/home/alice/proj" in out, out
          assert "33 101" in out, out  # the terminal's size reached the agent
          assert "HOME=/var/lib/starah-agents/tester/home/alice" in out, out
          assert "secret" not in out, out  # the rest of the home is hidden
          assert "/etc/hostname" in out, out  # its home link
          # What it made is alice's on disk.
          assert machine.succeed("stat -c %U /home/alice/proj/made.txt").strip() == "alice"
          # The session's mount is gone once the agent has exited.
          machine.wait_until_succeeds("test -z \"$(ls -A /run/starah/sessions 2>/dev/null)\"", timeout=30)
          machine.fail("findmnt -rn | grep -q /run/starah/sessions")

      with subtest("interactive: not in the home itself, unless the person allowed it"):
          out = machine.fail(alice("/etc/starah-test/session /home/alice 2>&1"))
          machine.succeed(alice("starah home tester on"))
          out = machine.succeed(alice("/etc/starah-test/session /home/alice/proj"))
          assert "secret" in out and "HOME=/home/alice" in out, out
          assert '"event":"home-access"' in machine.succeed(alice("starah audit 50"))
          machine.succeed(alice("starah home tester off"))

      with subtest("interactive: the agent stops when its terminal goes away"):
          # A terminal of alice's running the client (its own unit, so the test can close it).
          machine.succeed(
              "systemd-run --unit=alice-terminal --uid=alice -p WorkingDirectory=/home/alice/proj -p Environment=PATH=/run/current-system/sw/bin:/run/wrappers/bin "
              "script -qec 'starah run -i --agent tester -- /run/current-system/sw/bin/sleep 300' /dev/null"
          )
          machine.wait_until_succeeds("systemctl list-units --no-legend 'starah-tester-*' | grep -q running", timeout=30)
          machine.succeed("systemctl stop alice-terminal.service")
          machine.wait_until_fails("systemctl list-units --no-legend 'starah-tester-*.service' | grep -v prepare | grep -v end | grep -q starah", timeout=30)
          machine.wait_until_succeeds("test -z \"$(ls -A /run/starah/sessions 2>/dev/null)\"", timeout=30)
  '';
}
