# Starah

Root/User privilege level abstraction management layer for users and sysadmins,
controlling what an AI agent has access to. Starah ("Levels" in Sanskrit) intends to
solve the usability vs. security problem of AI agents in Linux distros.

*Starah* (Sanskrit: “levels”) gives AI agents **levels of privilege** on Linux. The
person who owns the machine chooses, at first login, what each agent may do on its own.
Anything beyond that level waits for the person's approval. No kernel changes are
needed: Starah is a broker built from polkit, systemd's sandbox and separate accounts per
agent.

| Level | Name | The agent may, on its own |
|---|---|---|
| 0 | Observe | look, never change |
| 1 | Propose | nothing; every change is a request you approve |
| 2 | Routine | changes a rollback fully undoes (installing software, toggling features, browser extensions, mounting disks) |
| 3 | Administer | edit configuration freely, manage services |
| 4 | Delegate | everything except Starah itself, accounts and passwords |

## Use (NixOS)

```nix
{
  inputs.starah.url = "github:lukskrp/Starah";
  # in your configuration:
  imports = [ starah.nixosModules.default ];
  services.starah = {
    enable = true;
    agents.assistant.description = "My coding agent";   # level 1 until you choose
  };
}
```

```sh
starah run --agent assistant -- my-agent --flag   # run an agent, sandboxed, as starah-assistant
starah status                                     # agents and levels
starah requests; starah approve 3                 # answer requests from a terminal
starah audit 50                                   # what agents asked and what was decided
starah rules edit                                 # your own rules (a Rhai script), checked before use
```

In a graphical session, `starah-session` handles all of this. It shows the setup at first
login, a summary at later logins, and a window for each request.

## Layout
- `crates/starah-policy`: the policy engine (levels, classes, requests, grants).
- `crates/starahd`: the broker (`org.starah.Broker1`).
- `crates/starah`: the CLI and `starah-polkit-check`.
- `apps/starah-session`: the OTK window.
- `data/`: D-Bus policy, polkit actions, action classes.
- `nix/`: packages, the NixOS module and the VM test.

See [docs/DESIGN.md](docs/DESIGN.md) for the architecture and the threat model. Starah
was made for [TulsiOS](https://github.com/lukskrp/TulsiOS) and works on any NixOS
system that has OTK from tulsipkgs.
