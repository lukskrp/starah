"""UI-free pieces: level texts, request formatting and the user's preferences."""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from pathlib import Path

from .i18n import _

# Must match starah-policy's Level (0–4).
LEVELS = [
    # Translators: the five privilege levels; short names shown in a menu and as
    # table headings ("2 · Routine").
    (_("Observe"), _("Can look at the system, but cannot change anything.")),
    (_("Propose"), _("Can suggest changes; you approve each one before it happens.")),
    (_("Routine"), _("Makes changes that a restart into the previous system can undo on its "
                     "own (installing software, turning features on or off, browser "
                     "extensions); asks you before anything that touches accounts or disks.")),
    (_("Administer"), _("Also edits the configuration freely and manages services on its "
                        "own; asks you before sensitive changes.")),
    (_("Delegate"), _("Does everything an administrator can on its own, except changing "
                      "Starah, user accounts and passwords.")),
]

# Translators: what kind of permission an agent asks for, in "Permission: ACTION — KIND".
CLASS_TEXT = {
    "observe": _("read-only"),
    "routine": _("low-risk"),
    "admin": _("administration"),
    "sensitive": _("sensitive"),
    "forbidden": _("never delegated"),
}


def level_title(level: int) -> str:
    name, _text = LEVELS[level]
    return f"{level} · {name}"


# ------------------------------------------------------------------ what a level allows

# The kinds of things an agent can ask for (starah-policy's Class), with examples
# from data/classes.json, in the order the window shows them.
# Translators: the row headings of the Compare Levels table (bold) and the examples
# under them (bullet points, lowercase, no final period).
CLASSES = [
    ("observe", _("Look around"), [
        _("read logs, settings and the system's status"),
    ]),
    ("routine", _("Changes a restart can undo"), [
        _("install and remove software"),
        _("turn features and services on or off (a NixOS rollback undoes it)"),
        _("add and remove browser extensions"),
        _("mount and eject USB sticks and disks"),
    ]),
    ("admin", _("System administration"), [
        _("edit the system configuration beyond that: user groups, boot, file systems, "
          "scripts"),
        _("start, stop and restart services"),
        _("network and Wi-Fi settings, the time, the computer's name and language"),
        _("power off, restart or suspend the computer"),
    ]),
    ("sensitive", _("Sensitive"), [
        _("write service definitions (systemd unit files) and system environment "
          "variables"),
        _("format and partition disks"),
        _("virtual machines and containers"),
        _("anything Starah doesn't recognise"),
    ]),
    ("forbidden", _("Never, at any level"), [
        _("change Starah or its levels"),
        _("create or change user accounts and passwords"),
        _("run programs as an administrator (pkexec)"),
    ]),
]

ALLOW, ASK, DENY = "allow", "ask", "deny"


def verdict(level: int, klass: str) -> str:
    """The level × class table; must match starah-policy's `verdict`."""
    if klass == "forbidden":
        return DENY
    if klass == "observe":
        return ALLOW
    if level == 0:
        return DENY
    if level == 1:
        return ASK
    if level == 2:
        return ALLOW if klass == "routine" else ASK
    if level == 3:
        return ASK if klass == "sensitive" else ALLOW
    return ALLOW


# Translators: the cells of the Compare Levels table and the headings of
# "What exactly?"; short (they sit under a ✓, ? or ✗ in a narrow column).
VERDICT_TEXT = {ALLOW: _("On its own"), ASK: _("Asks you first"), DENY: _("Not allowed")}
VERDICT_MARK = {ALLOW: "✓", ASK: "?", DENY: "✗"}


def level_breakdown(level: int) -> list[tuple[str, list[str]]]:
    """What an agent at `level` does on its own, asks about, and may not do:
    [(verdict, [examples…]), …], in that order, leaving out empty groups."""
    groups: dict[str, list[str]] = {ALLOW: [], ASK: [], DENY: []}
    for klass, _title, examples in CLASSES:
        groups[verdict(level, klass)].extend(examples)
    return [(v, groups[v]) for v in (ALLOW, ASK, DENY) if groups[v]]


def breakdown_markup(level: int) -> str:
    """The "What exactly?" text (Pango markup) for a level: on its own, asks first,
    not allowed."""
    from html import escape

    parts = []
    for v, examples in level_breakdown(level):
        items = "\n".join(f"   •  {escape(e, quote=False)}" for e in examples)
        parts.append(f"<b>{VERDICT_MARK[v]}  {VERDICT_TEXT[v]}</b>\n{items}")
    return "\n".join(parts)


@dataclass
class Agent:
    name: str
    description: str
    level: int
    default_level: int
    # People who let this agent use their whole home in interactive sessions.
    home_access: list[str] = field(default_factory=list)


@dataclass
class Policy:
    configured: bool
    agents: list[Agent]

    @classmethod
    def from_json(cls, text: str) -> "Policy":
        data = json.loads(text)
        return cls(
            configured=bool(data.get("configured")),
            agents=[
                Agent(
                    name=a["name"],
                    description=a.get("description", ""),
                    level=int(a["level"]),
                    default_level=int(a.get("default_level", a["level"])),
                    home_access=list(a.get("home_access", [])),
                )
                for a in data.get("agents", [])
            ],
        )


@dataclass
class Request:
    id: int
    agent: str
    level: int
    action: str
    klass: str
    kind: str
    detail: str | None
    title: str
    reason: str
    changes: list[str]
    diff: str | None

    @classmethod
    def from_json(cls, text: str) -> "Request":
        r = json.loads(text)
        d = r.get("description") or {}
        return cls(
            id=int(r["id"]),
            agent=r["agent"],
            level=int(r["level"]),
            action=r["action"],
            klass=r.get("class", "sensitive"),
            kind=r.get("kind", "polkit"),
            detail=r.get("detail"),
            title=d.get("title", ""),
            reason=d.get("reason", ""),
            changes=list(d.get("changes", [])),
            diff=d.get("diff"),
        )

    def headline(self) -> str:
        if self.title:
            return self.title
        what = self.action + (f" ({self.detail})" if self.detail else "")
        # Translators: {agent} is an agent's name, {what} a technical permission name.
        return _("Agent “{agent}” asks for: {what}").format(agent=self.agent, what=what)

    def summary_lines(self) -> list[str]:
        lines = [
            # Translators: {level} is like "2 · Routine".
            _("Agent: {agent} (level {level})").format(
                agent=self.agent, level=level_title(self.level)),
            # Translators: {action} is a technical name (org.tulsi.apply), {kind} one of
            # "read-only", "low-risk", "administration", "sensitive", "never delegated".
            _("Permission: {action} — {kind}").format(
                action=self.action, kind=CLASS_TEXT.get(self.klass, self.klass)),
        ]
        if self.detail:
            # Translators: what the permission is for, e.g. a service's name.
            lines.append(_("On: {detail}").format(detail=self.detail))
        if self.reason:
            lines.append(_("Reason given: {reason}").format(reason=self.reason))
        return lines

    @property
    def timed(self) -> bool:
        """A polkit-kind approval lasts some minutes; an Ask approval is one change."""
        return self.kind == "polkit"


def prefs_path() -> Path:
    base = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    return Path(base) / "starah" / "session.json"


def load_prefs(path: Path | None = None) -> dict:
    path = path or prefs_path()
    try:
        data = json.loads(path.read_text())
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def save_prefs(prefs: dict, path: Path | None = None) -> None:
    path = path or prefs_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(prefs, indent=2))
    tmp.replace(path)


def show_at_login(prefs: dict) -> bool:
    return bool(prefs.get("show_at_login", True))


# On TulsiOS, what to do when software someone installs has no package: package it
# with a recipe (no AI model), ask the Assistant (an AI agent), or ask each time.
# TulsiOS's tulsi-adopt reads this file; Starah only offers the setting there.
PACKAGING_CHOICES = (
    ("ask", _("Ask me each time")),
    ("recipe", _("Package it with a recipe (no AI model)")),
    ("assistant", _("Ask the Assistant (an AI agent)")),
)


def packaging_path() -> Path:
    base = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    return Path(base) / "tulsi" / "packaging.json"


def tulsi_packaging_available() -> bool:
    """TulsiOS's tulsi-adopt is installed (the setting means nothing elsewhere)."""
    import shutil

    return shutil.which("tulsi-adopt") is not None


def load_packaging(path: Path | None = None) -> str:
    choice = load_prefs(path or packaging_path()).get("whenNoPackage", "ask")
    return choice if choice in dict(PACKAGING_CHOICES) else "ask"


def save_packaging(choice: str, path: Path | None = None) -> None:
    if choice not in dict(PACKAGING_CHOICES):
        raise ValueError(choice)
    path = path or packaging_path()
    data = load_prefs(path)
    data["whenNoPackage"] = choice
    save_prefs(data, path)


# ------------------------------------------------------------------ the person's rules

# Terminals to run `starah rules edit` in, best first: TulsiOS's launcher, then the
# Debian-style default, then common ones. Each entry: program, then the arguments
# before the command.
TERMINALS = (
    ("tulsi-terminal", []),
    ("x-terminal-emulator", ["-e"]),
    ("gnome-terminal", ["--wait", "--"]),
    ("konsole", ["-e"]),
    ("xfce4-terminal", ["-x"]),
    ("st", ["-e"]),
    ("xterm", ["-e"]),
)

# The session service's PATH may not include the system's programs.
SYSTEM_BIN = ("/run/current-system/sw/bin",)


def find_program(name: str, which=None, exists=None) -> str | None:
    import shutil

    which = which or shutil.which
    exists = exists or os.path.exists
    found = which(name)
    if found:
        return found
    return next((f"{d}/{name}" for d in SYSTEM_BIN if exists(f"{d}/{name}")), None)


def terminal_command(argv: list[str], which=None, exists=None) -> list[str] | None:
    """argv run in a terminal window, or None when there is no terminal."""
    for program, before in TERMINALS:
        path = find_program(program, which, exists)
        if path:
            return [path, *before, *argv]
    return None


def edit_rules_command(which=None, exists=None) -> list[str] | None:
    """`starah rules edit` in a terminal (the Edit Rules… button)."""
    starah = find_program("starah", which, exists)
    if starah is None:
        return None
    return terminal_command([starah, "rules", "edit", "--pause-on-error"], which, exists)


def rules_summary(status: dict) -> str:
    """One line about the person's rules, from GetRules' status."""
    state = status.get("state")
    if state == "error":
        err = status.get("error") or {}
        message = err.get("message", "?")
        if err.get("line"):
            # Translators: {message} is the script error, in English.
            return _("Your rules have an error at line {line}: {message}. The rules before "
                     "it are still in use.").format(line=err["line"], message=message)
        return _("Your rules have an error: {message}. The rules before it are still in "
                 "use.").format(message=message)
    if state == "ok" and status.get("active"):
        return _("Your own rules refine these levels (they are in use).")
    return _("You can refine these levels with your own rules (a small script).")
