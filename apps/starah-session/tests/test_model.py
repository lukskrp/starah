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


def test_five_levels_match_the_broker():
    assert [name for name, _ in LEVELS] == [
        "Observe", "Propose", "Routine", "Administer", "Delegate"
    ]
    assert level_title(3) == "3 · Administer"


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
