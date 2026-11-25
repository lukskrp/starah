"""org.starah.Broker1 over Gio's D-Bus (no extra Python packages)."""

from __future__ import annotations

from typing import Callable

from gi.repository import Gio, GLib

NAME = "org.starah.Broker1"
PATH = "/org/starah/Broker1"
IFACE = "org.starah.Broker1"


class BrokerError(Exception):
    pass


def _message(err: GLib.Error) -> str:
    text = err.message or str(err)
    if Gio.DBusError.is_remote_error(err):
        Gio.DBusError.strip_remote_error(err)
        text = err.message
    return text


class Broker:
    def __init__(self) -> None:
        self.bus = Gio.bus_get_sync(Gio.BusType.SYSTEM, None)

    def _call_sync(self, method: str, params: GLib.Variant | None):
        try:
            return self.bus.call_sync(
                NAME, PATH, IFACE, method, params, None, Gio.DBusCallFlags.NONE, 10_000, None
            )
        except GLib.Error as e:
            raise BrokerError(_message(e)) from e

    def _call_async(self, method: str, params: GLib.Variant | None,
                    done: Callable[[GLib.Variant | None, str | None], None]) -> None:
        """For calls that may wait on a polkit password prompt."""
        def finish(bus, result):
            try:
                done(bus.call_finish(result), None)
            except GLib.Error as e:
                done(None, _message(e))

        self.bus.call(
            NAME, PATH, IFACE, method, params, None,
            Gio.DBusCallFlags.ALLOW_INTERACTIVE_AUTHORIZATION, 5 * 60_000, None, finish,
        )

    def get_policy(self) -> str:
        return self._call_sync("GetPolicy", None).unpack()[0]

    def requests(self) -> str:
        return self._call_sync("Requests", None).unpack()[0]

    def set_policy(self, levels_json: str, done: Callable[[str | None], None]) -> None:
        self._call_async("SetPolicy", GLib.Variant("(s)", (levels_json,)),
                         lambda _r, err: done(err))

    def set_home_access(self, agent: str, allow: bool, done: Callable[[str | None], None]) -> None:
        self._call_async("SetHomeAccess", GLib.Variant("(sb)", (agent, allow)),
                         lambda _r, err: done(err))

    def decide(self, rid: int, approve: bool, minutes: int,
               done: Callable[[str | None], None]) -> None:
        self._call_async("Decide", GLib.Variant("(tbu)", (rid, approve, minutes)),
                         lambda _r, err: done(err))

    def on_request_added(self, handler: Callable[[int, str], None]) -> int:
        def cb(_bus, _sender, _path, _iface, _signal, params):
            rid, request = params.unpack()
            handler(rid, request)
        return self.bus.signal_subscribe(
            NAME, IFACE, "RequestAdded", PATH, None, Gio.DBusSignalFlags.NONE, cb
        )

    def on_decided(self, handler: Callable[[int, bool, str], None]) -> int:
        def cb(_bus, _sender, _path, _iface, _signal, params):
            handler(*params.unpack())
        return self.bus.signal_subscribe(
            NAME, IFACE, "Decided", PATH, None, Gio.DBusSignalFlags.NONE, cb
        )
