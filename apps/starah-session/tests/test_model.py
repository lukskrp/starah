import json

from starah_session.model import (
    LEVELS,
    Policy,
    Request,
    level_title,
    load_prefs,
    save_prefs,
    show_at_login,
)


def test_tulsios_packaging_choice(tmp_path):
    from starah_session.model import load_packaging, save_packaging

    path = tmp_path / "packaging.json"
    assert load_packaging(path) == "ask"
    path.write_text('{"keep": true}')
    save_packaging("recipe", path)
    assert load_packaging(path) == "recipe"
    assert json.loads(path.read_text()) == {"keep": True, "whenNoPackage": "recipe"}
    path.write_text('{"whenNoPackage": "something"}')
    assert load_packaging(path) == "ask"


def test_five_levels_match_the_broker():
    assert [name for name, _ in LEVELS] == [
        "Observe", "Propose", "Routine", "Administer", "Delegate"
    ]
    assert level_title(3) == "3 · Administer"


def test_the_table_matches_the_broker():
    # The same table as starah-policy's `table` test (lib.rs).
    from starah_session.model import verdict

    expect = {
        0: ["allow", "deny", "deny", "deny", "deny"],
        1: ["allow", "ask", "ask", "ask", "deny"],
        2: ["allow", "allow", "ask", "ask", "deny"],
        3: ["allow", "allow", "allow", "ask", "deny"],
        4: ["allow", "allow", "allow", "allow", "deny"],
    }
    classes = ["observe", "routine", "admin", "sensitive", "forbidden"]
    for level, row in expect.items():
        assert [verdict(level, c) for c in classes] == row, level


def test_every_class_is_explained():
    from starah_session.model import CLASS_TEXT, CLASSES

    assert [c for c, _t, _e in CLASSES] == list(CLASS_TEXT)
    assert all(examples for _c, _t, examples in CLASSES)


def test_level_breakdown():
    from starah_session.model import level_breakdown

    routine = dict(level_breakdown(2))
    assert "install and remove software" in routine["allow"]
    assert "start, stop and restart services" in routine["ask"]
    assert "change Starah or its levels" in routine["deny"]
    # Observe: nothing is asked; Delegate: only the forbidden class is refused.
    assert [v for v, _ in level_breakdown(0)] == ["allow", "deny"]
    assert dict(level_breakdown(4))["deny"] == dict(level_breakdown(1))["deny"]


def test_breakdown_markup_is_pango_safe():
    from starah_session.model import breakdown_markup

    text = breakdown_markup(3)
    assert text.startswith("<b>✓  On its own</b>")
    assert "<b>?  Asks you first</b>" in text and "<b>✗  Not allowed</b>" in text
    assert "&" not in text.replace("&amp;", "").replace("&lt;", "").replace("&gt;", "")


def test_policy_from_broker_json():
    text = json.dumps({
        "configured": False,
        "agents": [{"name": "assistant", "description": "Ocimum Assistant", "level": 1,
                    "level_name": "Propose", "default_level": 1, "user": "starah-assistant"}],
        "levels": [],
    })
    p = Policy.from_json(text)
    assert not p.configured
    assert p.agents[0].name == "assistant" and p.agents[0].level == 1


def test_polkit_request_is_timed_and_names_the_action():
    r = Request.from_json(json.dumps({
        "id": 3, "agent": "assistant", "level": 1,
        "action": "org.freedesktop.systemd1.manage-units", "class": "admin",
        "kind": "polkit", "detail": "nginx.service", "created": 0, "status": "pending",
    }))
    assert r.timed
    assert "manage-units (nginx.service)" in r.headline()
    assert "On: nginx.service" in r.summary_lines()


def test_ask_request_shows_the_agents_description():
    r = Request.from_json(json.dumps({
        "id": 4, "agent": "assistant", "level": 2, "action": "org.tulsi.apply",
        "class": "admin", "kind": "ask", "created": 0, "status": "pending",
        "description": {"title": "An AI agent wants to change this system",
                        "reason": "The user asked for cowsay.",
                        "changes": ["+ package cowsay"]},
    }))
    assert not r.timed
    assert r.headline() == "An AI agent wants to change this system"
    assert r.changes == ["+ package cowsay"]
    assert "Reason given: The user asked for cowsay." in r.summary_lines()


def test_prefs_roundtrip_and_default(tmp_path):
    path = tmp_path / "starah" / "session.json"
    assert show_at_login(load_prefs(path))
    save_prefs({"show_at_login": False}, path)
    assert not show_at_login(load_prefs(path))
    path.write_text("not json")
    assert load_prefs(path) == {}


def test_edit_rules_runs_in_a_terminal():
    from starah_session.model import edit_rules_command

    progs = {"starah": "/bin/starah", "gnome-terminal": "/bin/gnome-terminal"}
    cmd = edit_rules_command(which=progs.get, exists=lambda _p: False)
    assert cmd == ["/bin/gnome-terminal", "--wait", "--", "/bin/starah", "rules", "edit", "--pause-on-error"]
    # TulsiOS's launcher first; the system's programs when PATH lacks them.
    cmd = edit_rules_command(
        which=lambda _n: None,
        exists=lambda p: p in ("/run/current-system/sw/bin/starah",
                               "/run/current-system/sw/bin/tulsi-terminal"),
    )
    assert cmd == ["/run/current-system/sw/bin/tulsi-terminal",
                   "/run/current-system/sw/bin/starah", "rules", "edit",
                   "--pause-on-error"]
    assert edit_rules_command(which=lambda _n: None, exists=lambda _p: False) is None


def test_rules_summary():
    from starah_session.model import rules_summary

    assert "in use" in rules_summary({"state": "ok", "active": True})
    assert "can refine" in rules_summary({"state": "ok", "active": False})
    assert "can refine" in rules_summary({"state": "none"})
    text = rules_summary({"state": "error", "error": {"message": "oops", "line": 12}})
    assert "line 12" in text and "oops" in text and "still in use" in text


def test_a_catalogue_translates_the_window(tmp_path):
    """With a starah.mo for the person's language, the strings are translated;
    without one (or for a string it lacks), they stay English."""
    import os
    import shutil
    import subprocess
    import sys

    import pytest

    if shutil.which("msgfmt") is None:
        pytest.skip("msgfmt (gettext) is not installed")
    po = tmp_path / "xx.po"
    po.write_text(
        'msgid ""\nmsgstr "Content-Type: text/plain; charset=UTF-8\\n"\n\n'
        'msgid "Compare Levels"\nmsgstr "Jämför nivåer"\n\n'
        'msgid "Propose"\nmsgstr "Föreslå"\n',
        encoding="utf-8",
    )
    mo = tmp_path / "locale" / "xx" / "LC_MESSAGES" / "starah.mo"
    mo.parent.mkdir(parents=True)
    subprocess.run(["msgfmt", "-o", str(mo), str(po)], check=True)
    code = (
        "from starah_session.i18n import _; from starah_session.model import LEVELS, VERDICT_TEXT;"
        "print(_('Compare Levels')); print(LEVELS[1][0]); print(VERDICT_TEXT['ask'])"
    )
    env = dict(os.environ, STARAH_LOCALEDIR=str(tmp_path / "locale"), LANGUAGE="xx",
               LC_ALL="", LANG="", PYTHONPATH=os.pathsep.join(sys.path))
    out = subprocess.run([sys.executable, "-c", code], env=env, capture_output=True,
                         text=True, check=True).stdout.splitlines()
    assert out == ["Jämför nivåer", "Föreslå", "Asks you first"], out
