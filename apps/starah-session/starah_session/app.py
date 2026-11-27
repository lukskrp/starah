"""starah-session: Starah in the human's desktop session (OTK).

Started at every login (a user service after graphical-session.target):
- while the levels have never been chosen, it opens **Starah Setup**;
- afterwards it shows the **Agent Privileges** summary at login (unless turned off);
- it stays running and shows an **Agent Request** window whenever an agent asks for
  something its level does not allow on its own.

`starah-session --show` (the menu entry) opens the summary in the running instance;
`starah-session --compare` shows what each level allows, side by side.
"""

from __future__ import annotations

import getpass
import json
import os
import shutil
import subprocess
import sys

import gi

gi.require_version("Otk", "3.0")
from gi.repository import Gio, GLib, Otk as Gtk  # noqa: E402

from .broker import Broker, BrokerError  # noqa: E402
from .i18n import _  # noqa: E402
from .model import (  # noqa: E402
    CLASSES,
    LEVELS,
    PACKAGING_CHOICES,
    VERDICT_MARK,
    VERDICT_TEXT,
    Policy,
    Request,
    breakdown_markup,
    edit_rules_command,
    level_title,
    load_packaging,
    load_prefs,
    rules_summary,
    save_packaging,
    save_prefs,
    show_at_login,
    tulsi_packaging_available,
    verdict,
)

APP_ID = "org.starah.Session"


def wrap_label(text: str, css: str | None = None) -> Gtk.Label:
    label = Gtk.Label(label=text, xalign=0)
    label.set_line_wrap(True)
    label.set_max_width_chars(70)
    if css:
        label.get_style_context().add_class(css)
    return label


class CompareWindow(Gtk.Window):
    """All five levels side by side: what each lets an agent do."""

    def __init__(self, parent: Gtk.Window | None):
        super().__init__(title=_("Compare Levels"), transient_for=parent)
        # Fits a 1024×768 screen; the table scrolls if the screen is smaller.
        self.set_default_size(960, 600)
        self.set_border_width(16)
        self.set_position(Gtk.WindowPosition.CENTER_ON_PARENT if parent
                          else Gtk.WindowPosition.CENTER)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.add(box)
        heading = Gtk.Label(xalign=0)
        heading.set_markup("<big><b>{}</b></big>".format(
            GLib.markup_escape_text(_("What each level lets an agent do"))))
        box.pack_start(heading, False, False, 0)
        box.pack_start(wrap_label(
            # Translators: the table's legend; keep {allow}, {ask} and {deny} (the
            # marks ✓, ? and ✗).
            _("{allow} on its own   ·   {ask} asks you first (you approve or reject each "
              "time)   ·   {deny} not allowed").format(
                allow=VERDICT_MARK["allow"], ask=VERDICT_MARK["ask"],
                deny=VERDICT_MARK["deny"])),
            False, False, 0)

        grid = Gtk.Grid(column_spacing=18, row_spacing=10, margin_end=12)
        scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER, vexpand=True)
        scroller.add(grid)
        box.pack_start(scroller, True, True, 0)
        for col, (name, _text) in enumerate(LEVELS, start=1):
            head = Gtk.Label()
            head.set_markup(f"<b>{col - 1} · {GLib.markup_escape_text(name)}</b>")
            grid.attach(head, col, 0, 1, 1)
        for row, (klass, title, examples) in enumerate(CLASSES, start=1):
            what = wrap_label("")
            what.set_max_width_chars(48)
            items = "\n".join(f"•  {GLib.markup_escape_text(e)}" for e in examples)
            what.set_markup(f"<b>{GLib.markup_escape_text(title)}</b>\n<small>{items}</small>")
            grid.attach(what, 0, row, 1, 1)
            for col in range(1, len(LEVELS) + 1):
                v = verdict(col - 1, klass)
                cell = Gtk.Label()
                cell.set_markup(f"<big>{VERDICT_MARK[v]}</big>\n<small>{VERDICT_TEXT[v]}</small>")
                cell.set_justify(Gtk.Justification.CENTER)
                # Translators: a table cell's tooltip, e.g. "Routine: Asks you first".
                cell.set_tooltip_text(_("{level}: {verdict}").format(
                    level=LEVELS[col - 1][0], verdict=VERDICT_TEXT[v]))
                grid.attach(cell, col, row, 1, 1)
        box.pack_start(wrap_label(
            # Translators: "Edit Rules…" is the button's name in Agent Privileges.
            _("Starah answers each request from the agent's level. Your own rules (Edit "
              "Rules…) can change the answer for particular agents and actions, but never "
              "allow what the last row forbids.")), False, False, 0)
        close = Gtk.Button.new_with_mnemonic(_("_Close"))
        close.set_halign(Gtk.Align.END)
        close.connect("clicked", lambda _b: self.destroy())
        box.pack_start(close, False, False, 0)


class LevelsWindow(Gtk.ApplicationWindow):
    """First-login setup (setup=True) or the login summary."""

    def __init__(self, app: "StarahApp", policy: Policy, setup: bool):
        super().__init__(application=app)
        self.app = app
        self.setup = setup
        self.set_title(_("Starah Setup") if setup else _("Agent Privileges"))
        self.set_default_size(620, -1)
        self.set_position(Gtk.WindowPosition.CENTER)
        self.set_border_width(20)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.add(box)
        heading = Gtk.Label(xalign=0)
        heading.set_markup("<big><b>{}</b></big>".format(GLib.markup_escape_text(
            _("Choose what AI agents may do") if setup else _("AI agents on this computer"))))
        box.pack_start(heading, False, False, 0)
        box.pack_start(wrap_label(
            _("Agents run as their own restricted accounts. When an agent needs a "
              "privilege, Starah answers for you from the level you choose here, and asks "
              "you whenever the level says so. You can change this at any time from "
              "“Agent Privileges”.")
            if setup else
            _("These are the privilege levels you chose. Agents ask you for anything their "
              "level does not allow on its own.")
        ), False, False, 0)

        grid = Gtk.Grid(column_spacing=12, row_spacing=6, margin_end=12)
        # Many agents scroll, so the window (and its Save button) stays on screen.
        scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER,
                                      propagate_natural_height=True, max_content_height=380)
        scroller.add(grid)
        box.pack_start(scroller, True, True, 6)

        # Starah covers the agents the system starts through it; say so.
        warning = wrap_label(
            _("⚠ This covers agents your system starts under Starah. An agent you installed "
              "some other way (npm, pip, a script from a website, a downloaded program) "
              "runs as you, with access to all your files."))
        box.pack_start(warning, False, False, 0)
        # (The session service's PATH may not include the system's programs.)
        wiki = shutil.which("tulsi-wiki") or next(
            (p for p in ("/run/current-system/sw/bin/tulsi-wiki",) if os.path.exists(p)), None)
        links = Gtk.Box(spacing=8)
        compare = Gtk.Button(label=_("Compare Levels"))
        compare.set_tooltip_text(_("What each level lets an agent do, side by side"))
        compare.connect("clicked", lambda _b: CompareWindow(self).show_all())
        links.pack_start(compare, False, False, 0)
        if wiki:
            more = Gtk.Button(label=_("Learn More"))
            more.connect("clicked", lambda _b: subprocess.Popen(
                [wiki, "ai-agents"], start_new_session=True))
            links.pack_start(more, False, False, 0)
        # The person's own rules (a Rhai script) refine the levels.
        self.rules_label = None
        edit_cmd = edit_rules_command()
        if not setup and edit_cmd:
            edit = Gtk.Button(label=_("Edit Rules…"))
            edit.set_tooltip_text(
                _("Opens your rules (/etc/starah/rules.rhai) in a terminal editor; they are "
                  "checked before they are used, and saving asks for the administrator "
                  "password."))
            edit.connect("clicked", lambda _b: subprocess.Popen(edit_cmd, start_new_session=True))
            links.pack_start(edit, False, False, 0)
            self.rules_label = wrap_label("")
            self.rules_label.get_style_context().add_class("dim-label")
            self._refresh_rules()
            # After editing in the terminal, the status shows the result.
            self.connect("focus-in-event", lambda *_a: self._refresh_rules())
        box.pack_start(links, False, False, 0)
        if self.rules_label is not None:
            box.pack_start(self.rules_label, False, False, 0)
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
            # What exactly the chosen level lets it do, ask about, and never do.
            details = wrap_label("")
            details.set_margin_start(12)
            details.set_markup(breakdown_markup(agent.level))
            exactly = Gtk.Expander(label=_("What exactly?"))
            exactly.add(details)
            combo.connect("changed", self._on_level_changed, explain, details)
            row += 1
            if agent.description:
                grid.attach(wrap_label(agent.description), 0, row, 2, 1)
                row += 1
            grid.attach(explain, 0, row, 2, 1)
            row += 1
            grid.attach(exactly, 0, row, 2, 1)
            row += 1
            # What an agent started in a terminal may see of the person's files.
            home = Gtk.CheckButton(label=_("May use my whole home folder"))
            home.set_active(getpass.getuser() in agent.home_access)
            home_note = wrap_label(
                _("Off: it sees only the folder you start it in, and has its own logins. "
                  "On: it works as if it were you in your files — your logins and "
                  "settings, your SSH keys (it can push with git), your other projects. Its "
                  "changes to the system still follow its level."))
            home_note.get_style_context().add_class("dim-label")
            home.connect("toggled", self._on_home_toggled, agent.name)
            grid.attach(home, 0, row, 2, 1)
            row += 1
            grid.attach(home_note, 0, row, 2, 1)
            row += 1
            self.combos[agent.name] = combo
            self.initial[agent.name] = agent.level

        # On TulsiOS: packaging software that has no package, with or without an agent.
        if tulsi_packaging_available():
            title = Gtk.Label(xalign=0)
            title.set_markup("<b>{}</b>".format(GLib.markup_escape_text(
                _("When software you install has no TulsiOS package"))))
            box.pack_start(title, False, False, 0)
            packaging = Gtk.ComboBoxText()
            for key, label in PACKAGING_CHOICES:
                packaging.append(key, label)
            packaging.set_active_id(load_packaging())
            packaging.connect(
                "changed", lambda c: save_packaging(c.get_active_id() or "ask"))
            box.pack_start(packaging, False, False, 0)
            box.pack_start(wrap_label(
                _("A recipe packages common downloads (AppImages, .deb and .rpm files, "
                  "programs and source code in archives) from tested templates, without an "
                  "AI model. The Assistant can package almost anything, but needs one.")
            ), False, False, 0)

        self.message = wrap_label("")
        box.pack_start(self.message, False, False, 0)

        self.show_login = Gtk.CheckButton(label=_("Show this when I log in"))
        self.show_login.set_active(show_at_login(load_prefs()))
        self.show_login.connect("toggled", self._on_show_login)
        if not setup:
            box.pack_start(self.show_login, False, False, 0)

        buttons = Gtk.ButtonBox(layout_style=Gtk.ButtonBoxStyle.END, spacing=8)
        box.pack_end(buttons, False, False, 0)
        if not setup:
            close = Gtk.Button.new_with_mnemonic(_("_Close"))
            close.connect("clicked", lambda _b: self.destroy())
            buttons.add(close)
        # Translators: buttons; the _ marks the access key (Alt+S).
        self.save = Gtk.Button.new_with_mnemonic(
            _("_Save") if not setup else _("_Save and continue"))
        self.save.get_style_context().add_class("suggested-action")
        self.save.connect("clicked", self._on_save)
        self.save.set_sensitive(setup)
        buttons.add(self.save)
        self.show_all()

    def _refresh_rules(self) -> bool:
        if self.rules_label is None:
            return False
        try:
            status = json.loads(self.app.broker.get_rules()).get("status", {})
        except (BrokerError, ValueError):
            status = {}
        self.rules_label.set_text(rules_summary(status))
        return False

    def _on_level_changed(self, combo: Gtk.ComboBoxText, explain: Gtk.Label,
                          details: Gtk.Label) -> None:
        explain.set_text(LEVELS[combo.get_active()][1])
        details.set_markup(breakdown_markup(combo.get_active()))
        changed = any(c.get_active() != self.initial[n] for n, c in self.combos.items())
        self.save.set_sensitive(self.setup or changed)

    def _on_home_toggled(self, check: Gtk.CheckButton, agent: str) -> None:
        allow = check.get_active()

        def done(err: str | None) -> None:
            if err:
                # Translators: {error} is a system message, often in English.
                self.message.set_text(_("Not changed: {error}").format(error=err))
                check.handler_block_by_func(self._on_home_toggled)
                check.set_active(not allow)
                check.handler_unblock_by_func(self._on_home_toggled)
            else:
                self.message.set_text(
                    (_("{agent} may now use your whole home folder.") if allow
                     else _("{agent} no longer uses your whole home folder.")).format(
                        agent=agent))

        self.app.broker.set_home_access(agent, allow, done)

    def _on_show_login(self, check: Gtk.CheckButton) -> None:
        prefs = load_prefs()
        prefs["show_at_login"] = check.get_active()
        save_prefs(prefs)

    def _on_save(self, _button) -> None:
        levels = {name: c.get_active() for name, c in self.combos.items()}
        self.save.set_sensitive(False)
        self.message.set_text(_("Saving… (you may be asked for your password)"))

        def done(err: str | None) -> None:
            if err:
                self.message.set_text(_("Not saved: {error}").format(error=err))
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
        self.set_title(_("Agent Request"))
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
            box.pack_start(wrap_label(_("Changes:")), False, False, 0)
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
            # Translators: "Allow for [10] minutes", around a number field.
            row.pack_start(Gtk.Label(label=_("Allow for")), False, False, 0)
            row.pack_start(self.minutes, False, False, 0)
            row.pack_start(Gtk.Label(label=_("minutes")), False, False, 0)
            box.pack_start(row, False, False, 0)

        self.message = wrap_label("")
        box.pack_start(self.message, False, False, 0)
        buttons = Gtk.ButtonBox(layout_style=Gtk.ButtonBoxStyle.END, spacing=8)
        box.pack_end(buttons, False, False, 0)
        reject = Gtk.Button.new_with_mnemonic(_("_Reject"))
        reject.connect("clicked", lambda _b: self._decide(False))
        approve = Gtk.Button.new_with_mnemonic(_("_Approve"))
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
                self.message.set_text(_("Could not answer: {error}").format(error=err))
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


def compare_main() -> int:
    """`starah-session --compare`: the levels table on its own (no broker needed)."""
    app = Gtk.Application(application_id=APP_ID + ".Compare",
                          flags=Gio.ApplicationFlags.NON_UNIQUE)

    def activate(a: Gtk.Application) -> None:
        window = CompareWindow(None)
        a.add_window(window)
        window.show_all()
        window.present()

    app.connect("activate", activate)
    return app.run([sys.argv[0]])


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--compare" in argv:
        return compare_main()
    first = "show" if "--show" in argv else "login"
    app = StarahApp(first)
    return app.run([sys.argv[0]])


if __name__ == "__main__":
    sys.exit(main())
