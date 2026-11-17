# Starah — design

*Starah* (Sanskrit: “levels”) lets the human who owns a Linux machine delegate
privileges that are normally gated by root to AI agents, in five levels, and keeps the
human in charge of everything above the chosen level.

## Does this need a modified kernel?

No. Linux already splits “root” into parts that a userspace broker can hand out:

| Need | Existing mechanism |
|---|---|
| Tell an agent from the human, unforgeably | A separate UID per agent. The kernel stamps it on every process and on every socket peer (`SO_PEERCRED`), which is how D-Bus and polkit learn who is asking. |
| Decide privileged operations | polkit. systemd, logind, NetworkManager, udisks, accountsservice, TulsiOS's tulsid and most desktop daemons ask polkit before doing anything privileged, and polkit's rules can hand the decision to a program (`polkit.spawn`). |
| Keep an agent from going around the decision | systemd's sandbox: `NoNewPrivileges` (setuid `sudo`/`su`/`pkexec` cannot raise privileges), an empty capability bounding set, read-only system, no home directories, seccomp-based protections. |
| Finer rules later | Landlock and BPF-LSM (both in mainline, unprivileged or loadable), not needed for this version. |

A kernel change would only be needed for new kernel object types or for intercepting
operations that never pass through a daemon. Starah's design keeps every privileged
operation behind a daemon that asks polkit, so it doesn't need one.

## Model

- **Agents** are declared by the system configuration. Agent `x` is the system user
  `starah-x` in group `starah-agents`, with a workspace `/var/lib/starah-agents/x`.
- **Levels**:

  | Level | Name | On its own | Asks the human | Never |
  |---|---|---|---|---|
  | 0 | Observe | read-only actions | — | everything else |
  | 1 | Propose | read-only actions | routine, admin, sensitive | forbidden |
  | 2 | Routine | + routine | admin, sensitive | forbidden |
  | 3 | Administer | + admin | sensitive | forbidden |
  | 4 | Delegate | + sensitive | — | forbidden |

- **Classes**: every polkit action id is in one class (`data/classes.json`). Exact ids
  win over prefixes, the longest prefix wins, and unknown actions are *sensitive*.
  - **observe**: reads state only.
  - **routine**: low-risk and easily undone, such as browser extensions or mounting a disk.
    On TulsiOS this includes installing software and turning features on or off
    (`org.tulsi.apply`): such a change lives in a NixOS generation, so `tulsi undo` or
    booting the previous generation undoes it completely. The human is asked about what a
    rollback cannot undo, not about what sounds risky.
  - **admin**: system administration: free-form configuration edits that reach outside a
    generation (`org.tulsi.configure`: accounts, boot, file systems, activation scripts,
    service code), services, network, power.
  - **sensitive**: can take over the machine or its data: unit files, raw disks,
    containers and VMs.
  - **forbidden**: never delegated, at any level: Starah itself (`org.starah.*`),
    accounts and passwords, `pkexec`.
- **Requests and grants**: when the level says “ask”, the attempt is refused and a
  request is filed. When the human approves:
  - a refused polkit action (**polkit** kind) is granted for N minutes, limited to the
    same detail (for systemd, the same unit); the agent's retry then succeeds;
  - a change the agent described beforehand (**ask** kind, used by tulsi-mcp) is granted
    for exactly one use.

  Requests expire after 15 minutes. Changing levels drops all grants.

## Components

```
 human session                    system                          agent (sandboxed)
 ─────────────                    ──────                          ─────────────────
 starah-session ──SetPolicy──▶  starahd (root)  ◀──Ask──────────  tulsi-mcp / any program
  (setup, summary,  ◀─RequestAdded─  │  ▲                           │
   Agent Request)  ──Decide──▶       │  └─Check── starah-polkit-check ◀─ polkit rule
                                     │                                     ▲
 starah run ──Run(fds)──▶ systemd transient unit (User=starah-x,          │
                          NoNewPrivileges, ProtectSystem=strict, …) ──────┘
                                                    privileged call ─▶ daemon ─▶ polkit
```

- **starahd** (`org.starah.Broker1`, system bus, root):
  - holds the levels (`/var/lib/starah/policy.json`), pending requests and grants;
  - writes every decision to `/var/lib/starah/audit.jsonl` and to the journal;
  - identifies callers by the uid the bus reports: root, polkitd's user, an agent, or a
    human. Agents can never call SetPolicy, Decide, Run, Requests or AuditLog.
  - Humans need the polkit actions `org.starah.configure` (always the admin password),
    `org.starah.decide` and `org.starah.run`. The NixOS option `adminsWithoutPassword`
    lets active local wheel members use the last two without a prompt.
- **The polkit rule** (`/etc/polkit-1/rules.d/00-starah.rules`):
  - It runs before all other rules. For any subject in `starah-agents` it spawns
    `starah-polkit-check ACTION USER DETAIL`, which asks starahd's `Check`.
  - The answer is final (YES or NO), so no later rule can give an agent more.
  - Any failure (starahd down, timeout) answers NO.
- **starah run**:
  - starts a program as an agent: starahd creates a transient systemd service with the
    caller's stdin/stdout/stderr (file descriptors passed over D-Bus);
  - `starah run` waits for the program and exits with its status;
  - this is how the Ocimum Assistant runs tulsi-mcp.
- **starah-session** (OTK), a user service in every graphical session:
  - **Starah Setup** while the levels were never confirmed;
  - **Agent Privileges** at later logins (it can be turned off) and from the menu;
  - an **Agent Request** window for every request, with the agent, permission, reason,
    changes and diff. Reject is the default.

## Threat model

**Protected:**
- An agent process cannot gain privileges beyond its level by:
  - running `sudo`, `su`, `pkexec` or other setuid programs (`NoNewPrivileges`,
    `RestrictSUIDSGID`);
  - using capabilities (empty bounding set);
  - writing system files (`ProtectSystem=strict`; its workspace is the only writable
    place);
  - reading home directories (`ProtectHome`);
  - pretending to be the human: the kernel's uid on the socket decides, and agents
    cannot call Starah's human-only methods;
  - talking polkit into it through other rules: Starah's rule is first and final for
    agent accounts.
- Every authorization question about an agent, every request, every decision and every
  agent start is in the audit log.

**Not protected, by design or for later:**
- **Privileged operations that do not go through polkit.** An example is a daemon that
  trusts any local caller. Starah cannot see those; the sandbox limits what the agent can
  reach. Landlock/BPF-LSM rules per agent are the next step.
- **Network access.** Agents can use the network (they need their model API). Network
  policy per agent is not in this version.
- **Data the human gives the agent.** Anything placed in the agent's workspace or passed
  on its stdin is the agent's.
- **Agents started outside Starah.** A program the human runs as themselves has the
  human's privileges. Starah covers agents started with `starah run` (or as configured
  services).
- **A human who approves everything.** Requests show what will happen. Rejecting is the
  default, but the decision is the human's.

## Prototype status (2026-11-14)
- **Done:**
  - the policy engine (`starah-policy`, unit-tested level × class table, grants and
    request lifecycle);
  - starahd, `starah`, `starah-polkit-check`;
  - the NixOS module;
  - starah-session (OTK);
  - the VM test `checks.agents`, which uses real systemd and polkit.
- **TulsiOS:**
  - the Ocimum Assistant's tulsi-mcp runs as agent `assistant` (`--approval starah`);
  - the Ocimum Desktop enables Starah;
  - the setup window appears at first login.
- **Next:**
  - Landlock file rules per agent and network policy;
  - agents as long-running services (`starah-agent@`);
  - a panel indicator for pending requests;
  - more action classes (CUPS, Flatpak, firmware updates);
  - several administrators.
