"""starah-session: Starah in the human's desktop session (OTK).

Started at every login (a user service after graphical-session.target):
- while the levels have never been chosen, it opens **Starah Setup**;
- afterwards it shows the **Agent Privileges** summary at login (unless turned off);
- it stays running and shows an **Agent Request** window whenever an agent asks for
  something its level does not allow on its own.

`starah-session --show` (the menu entry) opens the summary in the running instance.
"""

from __future__ import annotations

import json
import sys

import gi

gi.require_version("Otk", "3.0")
from gi.repository import Gio, GLib, Otk as Gtk  # noqa: E402

from .broker import Broker, BrokerError  # noqa: E402
from .model import (  # noqa: E402
    LEVELS,
    Policy,
    Request,
    level_title,
    load_prefs,
    save_prefs,
    show_at_login,
)

APP_ID = "org.starah.Session"


def wrap_label(text: str, css: str | None = None) -> Gtk.Label:
    label = Gtk.Label(label=text, xalign=0)
    label.set_line_wrap(True)
    label.set_max_width_chars(70)
    if css:
        label.get_style_context().add_class(css)
    return label


class LevelsWindow(Gtk.ApplicationWindow):
    """First-login setup (setup=True) or the login summary."""

    def __init__(self, app: "StarahApp", policy: Policy, setup: bool):
        super().__init__(application=app)
        self.app = app
        self.setup = setup
        self.set_title("Starah Setup" if setup else "Agent Privileges")
        self.set_default_size(620, -1)
        self.set_position(Gtk.WindowPosition.CENTER)
        self.set_border_width(20)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.add(box)
        heading = Gtk.Label(xalign=0)
        heading.set_markup(
            "<big><b>Choose what AI agents may do</b></big>" if setup
            else "<big><b>AI agents on this computer</b></big>"
        )
        box.pack_start(heading, False, False, 0)
        box.pack_start(wrap_label(
            "Agents run as their own restricted accounts. When an agent needs a privilege, "
            "Starah answers for you from the level you choose here, and asks you whenever "
            "the level says so. You can change this at any time from “Agent Privileges”."
            if setup else
            "These are the privilege levels you chose. Agents ask you for anything their "
            "level does not allow on its own."
        ), False, False, 0)

        grid = Gtk.Grid(column_spacing=12, row_spacing=6)
        box.pack_start(grid, False, False, 6)
        self.combos: dict[str, Gtk.ComboBoxText] = {}
        self.initial: dict[str, int] = {}
        row = 0
        for agent in policy.agents:
            name = Gtk.Label(xalign=0)
            name.set_markup(f"<b>{GLib.markup_escape_text(agent.name)}</b>")
            grid.attach(name, 0, row, 1, 1)
            combo = Gtk.ComboBoxText()
            for level in range(len(LEVELS)):
                combo.append(str(level), level_title(level))
            combo.set_active(agent.level)
            grid.attach(combo, 1, row, 1, 1)
            explain = wrap_label(LEVELS[agent.level][1])
            explain.get_style_context().add_class("dim-label")
            combo.connect("changed", self._on_level_changed, explain)
            row += 1
            if agent.description:
                grid.attach(wrap_label(agent.description), 0, row, 2, 1)
                row += 1
            grid.attach(explain, 0, row, 2, 1)
            row += 1
            self.combos[agent.name] = combo
            self.initial[agent.name] = agent.level

        self.message = wrap_label("")
        box.pack_start(self.message, False, False, 0)

        self.show_login = Gtk.CheckButton(label="Show this when I log in")
        self.show_login.set_active(show_at_login(load_prefs()))
        self.show_login.connect("toggled", self._on_show_login)
        if not setup:
            box.pack_start(self.show_login, False, False, 0)

        buttons = Gtk.ButtonBox(layout_style=Gtk.ButtonBoxStyle.END, spacing=8)
        box.pack_end(buttons, False, False, 0)
        if not setup:
            close = Gtk.Button.new_with_mnemonic("_Close")
            close.connect("clicked", lambda _b: self.destroy())
            buttons.add(close)
        self.save = Gtk.Button.new_with_mnemonic("_Save" if not setup else "_Save and continue")
        self.save.get_style_context().add_class("suggested-action")
        self.save.connect("clicked", self._on_save)
        self.save.set_sensitive(setup)
        buttons.add(self.save)
        self.show_all()

    def _on_level_changed(self, combo: Gtk.ComboBoxText, explain: Gtk.Label) -> None:
        explain.set_text(LEVELS[combo.get_active()][1])
        changed = any(c.get_active() != self.initial[n] for n, c in self.combos.items())
        self.save.set_sensitive(self.setup or changed)

    def _on_show_login(self, check: Gtk.CheckButton) -> None:
        prefs = load_prefs()
        prefs["show_at_login"] = check.get_active()
        save_prefs(prefs)

    def _on_save(self, _button) -> None:
        levels = {name: c.get_active() for name, c in self.combos.items()}
        self.save.set_sensitive(False)
        self.message.set_text("Saving… (you may be asked for your password)")

        def done(err: str | None) -> None:
            if err:
                self.message.set_text(f"Not saved: {err}")
                self.save.set_sensitive(True)
            else:
                print(f"starah-session: levels saved: {json.dumps(levels)}", flush=True)
                self.destroy()

        self.app.broker.set_policy(json.dumps(levels), done)


class RequestWindow(Gtk.ApplicationWindow):
    """One agent request. Reject is the default."""

    def __init__(self, app: "StarahApp", req: Request):
        super().__init__(application=app)
        self.app = app
        self.req = req
        self.set_title("Agent Request")
        self.set_default_size(560, -1)
        self.set_position(Gtk.WindowPosition.CENTER)
        self.set_keep_above(True)
        self.set_urgency_hint(True)
        self.set_border_width(18)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        self.add(box)
        heading = Gtk.Label(xalign=0)
        heading.set_markup(f"<big><b>{GLib.markup_escape_text(req.headline())}</b></big>")
        heading.set_line_wrap(True)
        box.pack_start(heading, False, False, 0)
        for line in req.summary_lines():
            box.pack_start(wrap_label(line), False, False, 0)
        if req.changes:
            box.pack_start(wrap_label("Changes:"), False, False, 0)
            for change in req.changes:
                box.pack_start(wrap_label(f"   • {change}"), False, False, 0)
        if req.diff:
            view = Gtk.TextView(editable=False, monospace=True)
            view.get_buffer().set_text(req.diff)
            scroll = Gtk.ScrolledWindow(min_content_height=180)
            scroll.add(view)
            box.pack_start(scroll, True, True, 0)

        self.minutes = Gtk.SpinButton.new_with_range(1, 240, 1)
        self.minutes.set_value(10)
        if req.timed:
            row = Gtk.Box(spacing=6)
            row.pack_start(Gtk.Label(label="Allow for"), False, False, 0)
            row.pack_start(self.minutes, False, False, 0)
            row.pack_start(Gtk.Label(label="minutes"), False, False, 0)
            box.pack_start(row, False, False, 0)

        self.message = wrap_label("")
        box.pack_start(self.message, False, False, 0)
        buttons = Gtk.ButtonBox(layout_style=Gtk.ButtonBoxStyle.END, spacing=8)
        box.pack_end(buttons, False, False, 0)
        reject = Gtk.Button.new_with_mnemonic("_Reject")
        reject.connect("clicked", lambda _b: self._decide(False))
        approve = Gtk.Button.new_with_mnemonic("_Approve")
        approve.get_style_context().add_class("suggested-action")
        approve.connect("clicked", lambda _b: self._decide(True))
        buttons.add(reject)
        buttons.add(approve)
        self.show_all()
        reject.grab_focus()

    def _decide(self, approve: bool) -> None:
        minutes = int(self.minutes.get_value()) if self.req.timed else 0

        def done(err: str | None) -> None:
            if err:
                self.message.set_text(f"Could not answer: {err}")
            else:
                print(f"starah-session: request {self.req.id} "
                      f"{'approved' if approve else 'rejected'}", flush=True)
                self.destroy()

        self.app.broker.decide(self.req.id, approve, minutes, done)


class StarahApp(Gtk.Application):
    def __init__(self, first: str):
        super().__init__(application_id=APP_ID)
        self.first = first
        self.started = False
        self.broker: Broker | None = None
        self.request_windows: dict[int, RequestWindow] = {}
        self.levels_window: LevelsWindow | None = None

    def do_activate(self) -> None:
        if self.started:
            self.show_levels(force=True)
            return
        self.started = True
        # Starah's own icon (the balance) for every window, in the window list too.
        Gtk.Window.set_default_icon_name(APP_ID)
        self.hold()  # keep running as the request notifier without windows
        try:
            self.broker = Broker()
            self.broker.on_request_added(lambda rid, text: self.show_request(text))
            self.broker.on_decided(self._on_decided)
        except Exception as e:  # no system bus / no starahd: nothing to do
            print(f"starah-session: {e}", file=sys.stderr)
            self.release()
            return
        self.show_levels(force=self.first == "show")
        try:
            for r in json.loads(self.broker.requests()):
                self.show_request(json.dumps(r))
        except (BrokerError, ValueError) as e:
            print(f"starah-session: {e}", file=sys.stderr)

    def show_levels(self, force: bool) -> None:
        try:
            policy = Policy.from_json(self.broker.get_policy())
        except BrokerError as e:
            print(f"starah-session: {e}", file=sys.stderr)
            return
        if self.levels_window is not None:
            self.levels_window.present()
            return
        if not policy.configured:
            window = LevelsWindow(self, policy, setup=True)
        elif force or show_at_login(load_prefs()):
            window = LevelsWindow(self, policy, setup=False)
        else:
            return
        self.levels_window = window
        window.connect("destroy", lambda _w: setattr(self, "levels_window", None))
        window.present()

    def show_request(self, text: str) -> None:
        req = Request.from_json(text)
        if req.id in self.request_windows:
            return
        window = RequestWindow(self, req)
        self.request_windows[req.id] = window
        window.connect("destroy", lambda _w: self.request_windows.pop(req.id, None))
        window.present()

    def _on_decided(self, rid: int, _approved: bool, _status: str) -> None:
        # Decided elsewhere (CLI, another window) or expired: close our window.
        window = self.request_windows.get(rid)
        if window is not None:
            window.destroy()


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    first = "show" if "--show" in argv else "login"
    app = StarahApp(first)
    return app.run([sys.argv[0]])


if __name__ == "__main__":
    sys.exit(main())
