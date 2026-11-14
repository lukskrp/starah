"""UI-free pieces: level texts, request formatting and the user's preferences."""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

# Must match starah-policy's Level (0–4).
LEVELS = [
    ("Observe", "Can look at the system, but cannot change anything."),
    ("Propose", "Can suggest changes; you approve each one before it happens."),
    ("Routine", "Makes low-risk changes on its own (browser extensions, mounting disks); "
                "asks you for everything else."),
    ("Administer", "Installs software, edits the configuration and manages services on its "
                   "own; asks you before sensitive changes."),
    ("Delegate", "Does everything an administrator can on its own, except changing "
                 "Starah, user accounts and passwords."),
]

CLASS_TEXT = {
    "observe": "read-only",
    "routine": "low-risk",
    "admin": "administration",
    "sensitive": "sensitive",
    "forbidden": "never delegated",
}


def level_title(level: int) -> str:
    name, _ = LEVELS[level]
    return f"{level} · {name}"


@dataclass
class Agent:
    name: str
    description: str
    level: int
    default_level: int


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
        return f"Agent “{self.agent}” asks for: {what}"

    def summary_lines(self) -> list[str]:
        lines = [
            f"Agent: {self.agent} (level {level_title(self.level)})",
            f"Permission: {self.action} — {CLASS_TEXT.get(self.klass, self.klass)}",
        ]
        if self.detail:
            lines.append(f"On: {self.detail}")
        if self.reason:
            lines.append(f"Reason given: {self.reason}")
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
