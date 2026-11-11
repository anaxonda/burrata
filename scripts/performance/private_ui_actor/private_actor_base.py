#!/usr/bin/env python3
"""Exact-control AT-SPI actions for the private saved-library visual phase."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import re
import sys
import time

PROFILE_RE = re.compile(r"^/tmp/burrata-launch\.[A-Za-z0-9_-]+$")
MAX_NODES = 12000
MAX_DEPTH = 48
PROFILE_SUMMARY = "Choose a profile"
DISCLOSURE_CANDIDATES = []
ACTIVATION = {}
ALBUM_CONTEXT = None
ALBUM_DESTINATION = {}


def album_context_from_cases(cases):
    plan = cases.get("libraryAlbumUi", {}) if isinstance(cases, dict) else {}
    title, artist = plan.get("albumTitle"), plan.get("albumArtist")
    if not all(isinstance(value, str) and value.strip() == value and 0 < len(value) <= 300
               and not any(ord(char) < 32 for char in value) for value in (title, artist)):
        raise ValueError("ALBUM_FIXTURE_CONTEXT_INVALID")
    incidents = cases.get("incidents")
    rows = [item for item in incidents if isinstance(item, dict) and item.get("album") == title] if isinstance(incidents, list) else []
    providers = {item.get("provenance", {}).get("catalogProvider") for item in rows}
    releases = {item.get("provenance", {}).get("releaseId") for item in rows}
    if len(rows) < 2 or len(providers) != 1 or len(releases) != 1 or not all(
            isinstance(value, str) and 0 < len(value) <= 300 for value in (*providers, *releases)):
        raise ValueError("ALBUM_FIXTURE_CONTEXT_INVALID")
    return {"title": title, "artist": artist, "sourceProvider": next(iter(providers)),
            "download": f"Download album options for {title}", "more": f"More album actions for {title}"}


def load_album_context(profile):
    path = profile / "fixture/runtime-cases.json"
    if path.is_symlink() or path.resolve(strict=True) != path or not path.is_file() or path.stat().st_size > 2 * 1024 * 1024:
        raise ValueError("ALBUM_FIXTURE_CONTEXT_INVALID")
    return album_context_from_cases(json.loads(path.read_text()))


def summary_kind(name):
    if name == PROFILE_SUMMARY:
        return "profile-summary"
    if ALBUM_CONTEXT:
        if name == ALBUM_CONTEXT["download"]: return "album-download"
        if name == ALBUM_CONTEXT["more"]: return "album-more"
    return None


def album_heading_attestation(obj, name):
    if not ALBUM_CONTEXT or name != ALBUM_CONTEXT["title"]:
        return False
    try:
        attrs = obj.get_attributes()
        return isinstance(attrs, dict) and attrs.get("toolkit") == "WebKitGTK" and attrs.get("tag") == "h1"
    except Exception:
        return False


def error_kind(error):
    if isinstance(error, TypeError): return "type-error"
    if isinstance(error, AttributeError): return "attribute-error"
    if type(error).__module__ in {"gi.repository.GLib", "gi._error"}: return "glib-error"
    return "other-error"


def summary_attestation(obj, name, role):
    kind = summary_kind(name)
    if not kind:
        return False
    evidence = {"target": kind, "rawRole": role if role in {"unknown", "invalid", "button", "push button", "toggle button", "text", "static"} else "other",
                "attributeShape": "error", "toolkit": "missing", "tag": "missing", "attested": False}
    try:
        attrs = obj.get_attributes()
        evidence["attributeShape"] = "dict" if isinstance(attrs, dict) else "other"
        if isinstance(attrs, dict):
            evidence["toolkit"] = attrs.get("toolkit", "missing") if attrs.get("toolkit", "missing") in {"WebKitGTK", "missing"} else "other"
            evidence["tag"] = attrs.get("tag", "missing") if attrs.get("tag", "missing") in {"summary", "button", "span", "missing"} else "other"
            evidence["attested"] = attrs.get("toolkit") == "WebKitGTK" and attrs.get("tag") == "summary"
    except Exception:
        pass
    if len(DISCLOSURE_CANDIDATES) < 8:
        DISCLOSURE_CANDIDATES.append(evidence)
    return evidence["attested"]


def fresh_state(obj, atspi):
    # Preserve full libatspi cache import; invalidate only the exact target.
    try:
        obj.clear_cache_single()
    except Exception:
        raise ValueError("TARGET_CONTROL_STATE_REFRESH_FAILED") from None
    return state_of(obj, atspi)


def pump_events(duration=.15, context=None):
    if context is None:
        from gi.repository import GLib
        context = GLib.MainContext.default()
    deadline = time.monotonic() + duration
    iterations = 0
    while time.monotonic() < deadline and iterations < 64:
        while context.pending() and iterations < 64:
            context.iteration(False)
            iterations += 1
        remaining = deadline - time.monotonic()
        if remaining > 0:
            time.sleep(min(.01, remaining))
    return iterations



def checked_context(profile: Path, pid: int, dbus: str, xdg: Path) -> str:
    if profile.is_symlink() or not PROFILE_RE.fullmatch(str(profile) or "") or profile.resolve(strict=True) != profile:
        raise ValueError("PRIVATE_PROFILE_INVALID")
    marker = profile / ".burrata-isolated-profile"
    if marker.is_symlink() or marker.read_text() != "burrata isolated memory run\n":
        raise ValueError("PRIVATE_PROFILE_MARKER_INVALID")
    if os.environ.get("DBUS_SESSION_BUS_ADDRESS") != dbus or os.environ.get("XDG_DATA_HOME") != str(xdg):
        raise ValueError("PRIVATE_ACTOR_ENVIRONMENT_MISMATCH")
    raw_env = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
    app_env = {key.decode(): value.decode(errors="replace") for item in raw_env if b"=" in item
               for key, value in [item.split(b"=", 1)]}
    expected_exe = Path(os.environ.get("BURRATA_QUAL_PRIVATE_APP_EXE", ""))
    if (Path(f"/proc/{pid}/exe").resolve() != expected_exe.resolve(strict=True)
            or app_env.get("DBUS_SESSION_BUS_ADDRESS") != dbus
            or app_env.get("XDG_DATA_HOME") != str(xdg)
            or app_env.get("BURRATA_APP_DATA") != str(profile / "data/org.burrata.discovery")
            or any(item.startswith(b"NO_AT_BRIDGE=") for item in raw_env)):
        raise ValueError("PRIVATE_APP_BINDING_INVALID")
    if xdg.resolve(strict=True) != (profile / "data").resolve(strict=True):
        raise ValueError("PRIVATE_DATA_HOME_INVALID")
    display = os.environ.get("DISPLAY", "")
    if app_env.get("DISPLAY") != display or not re.fullmatch(r":(?:9\d|10\d|11\d)", display):
        raise ValueError("PRIVATE_DISPLAY_INVALID")
    return display


def name_of(node):
    try:
        return (node.get_name() or "").strip()
    except Exception:
        return ""


def role_of(node):
    try:
        return (node.get_role_name() or "").strip().lower()
    except Exception:
        return ""


def state_of(node, atspi):
    try:
        state = node.get_state_set()
        return {"showing": bool(state.contains(atspi.StateType.SHOWING)),
                "enabled": bool(state.contains(atspi.StateType.ENABLED)),
                "checked": bool(state.contains(atspi.StateType.CHECKED)),
                "expanded": bool(state.contains(atspi.StateType.EXPANDED))}
    except Exception:
        return {"showing": False, "enabled": False, "checked": False, "expanded": False}


def rect_of(node, atspi):
    try:
        r = node.get_component_iface().get_extents(atspi.CoordType.SCREEN)
        return {"x": int(r.x), "y": int(r.y), "width": int(r.width), "height": int(r.height)}
    except Exception:
        return None


def collect(app, atspi):
    DISCLOSURE_CANDIDATES.clear()
    queue, out, visited = [(app, 0)], [], 0
    while queue and visited < MAX_NODES:
        node, depth = queue.pop(0); visited += 1
        name, role = name_of(node), role_of(node)
        attested_summary = summary_attestation(node, name, role)
        attested_heading = album_heading_attestation(node, name)
        if attested_summary:
            role = "summary"
        elif attested_heading:
            role = "heading"
        if name or role in {"button", "push button", "toggle button", "link", "page tab", "tab", "menu item"}:
            out.append({"object": node, "name": name, "role": role, "state": fresh_state(node, atspi) if attested_summary or attested_heading else state_of(node, atspi),
                        "summaryAttested": attested_summary, "albumHeadingAttested": attested_heading,
                        "bounds": rect_of(node, atspi)})
        if depth >= MAX_DEPTH:
            continue
        try:
            count = min(int(node.get_child_count()), MAX_NODES - visited)
        except Exception:
            count = 0
        for index in range(count):
            try:
                child = node.get_child_at_index(index)
                if child is not None:
                    queue.append((child, depth + 1))
            except Exception:
                continue
    if queue:
        raise ValueError("ACCESSIBILITY_TREE_NODE_LIMIT")
    return out


def app_accessible(pid, atspi):
    desktop = atspi.get_desktop(0)
    matches = []
    for index in range(min(int(desktop.get_child_count()), 128)):
        try:
            candidate = desktop.get_child_at_index(index)
            if int(candidate.get_process_id()) == pid:
                matches.append(candidate)
        except Exception:
            continue
    if len(matches) != 1:
        raise ValueError("PRIVATE_APP_ACCESSIBLE_AMBIGUOUS")
    return matches[0]


def is_showing(node):
    b = node["bounds"]
    return node["state"]["showing"] and isinstance(b, dict) and b["width"] > 0 and b["height"] > 0


def match_name(node, exact, contains, prefix=None):
    name = node["name"]
    if exact is not None:
        return name == exact
    if prefix is not None:
        return name.startswith(prefix)
    return all(part.casefold() in name.casefold() for part in contains)


def locate(nodes, exact, contains, roles, visible=True, prefix=None):
    found = [n for n in nodes if n["role"] in roles and match_name(n, exact, contains, prefix)
             and (is_showing(n) or not visible)]
    if len(found) != 1:
        raise ValueError("EXACT_VISIBLE_CONTROL_AMBIGUOUS_OR_MISSING")
    return found[0]


def click(node, atspi, app, exact, contains, roles, prefix=None, toggle_summary=None):
    ACTIVATION.clear()
    kind = summary_kind(exact) if not contains and prefix is None else None
    disclosure = bool(kind)
    album_disclosure = kind in {"album-download", "album-more"}
    ACTIVATION.update(target=kind or "other", stage="prepare",
        actionCount=0, names=[], method="none", attempted=False, accepted=False,
        semanticAttestation=False, expandedBefore=False, expandedAfter=False, expectedExpanded=None, stateVerified=False)
    if (album_disclosure and toggle_summary not in {"open", "closed"}) or (toggle_summary is not None and not album_disclosure):
        raise ValueError("ALBUM_DISCLOSURE_MODE_INVALID")
    if disclosure and not node.get("summaryAttested"):
        raise ValueError("PROFILE_DISCLOSURE_SEMANTICS_INVALID")
    ACTIVATION["semanticAttestation"] = disclosure and node.get("summaryAttested") is True
    node["state"] = fresh_state(node["object"], atspi)
    if not node["state"]["enabled"]:
        raise ValueError("TARGET_CONTROL_DISABLED")
    if not is_showing(node):
        ACTIVATION["stage"] = "reveal"
        component = node["object"].get_component_iface()
        if component is None or not bool(atspi.Component.scroll_to(component, atspi.ScrollType.ANYWHERE)):
            raise ValueError("TARGET_CONTROL_COULD_NOT_BE_REVEALED")
        pump_events()
        node = locate(collect(app, atspi), exact, contains, roles, visible=True, prefix=prefix)
        node["state"] = fresh_state(node["object"], atspi)
        if not is_showing(node):
            raise ValueError("TARGET_CONTROL_COULD_NOT_BE_REVEALED")
        if not node["state"]["enabled"]:
            raise ValueError("TARGET_CONTROL_DISABLED")
    ACTIVATION["expandedBefore"] = node["state"].get("expanded") is True
    expected_expanded = toggle_summary == "open" if album_disclosure else True
    ACTIVATION["expectedExpanded"] = expected_expanded if disclosure else None
    if album_disclosure and ACTIVATION["expandedBefore"] == expected_expanded:
        raise ValueError("ALBUM_DISCLOSURE_PRESTATE_MISMATCH")
    if kind == "profile-summary" and ACTIVATION["expandedBefore"]:
        # The caller needs the chooser open, not a second toggle which closes it.
        ACTIVATION.update(stage="verified", method="already-expanded", accepted=True, expandedAfter=True, stateVerified=True)
    else:
        ACTIVATION["stage"] = "action-interface"
        actions = node["object"].get_action_iface()
        ACTIVATION["stage"] = "action-count"
        count = int(atspi.Action.get_n_actions(actions))
        ACTIVATION["actionCount"] = max(0, min(count, 9))
        ACTIVATION["stage"] = "action-names"
        choices = [str(atspi.Action.get_action_name(actions, i)).strip().lower() for i in range(max(0, min(count, 8)))]
        known = {"click", "press", "activate", "toggle"}
        ACTIVATION["names"] = [value if value in known else "empty" if value == "" else "other" for value in choices]
        index = next((i for i, value in enumerate(choices) if value in known), None)
        if index is not None:
            ACTIVATION["method"] = "named-action"
        elif disclosure and node.get("summaryAttested") and count == 1 and choices == [""]:
            # WebKit 2.50.4 publishes one default action even when Summary has no
            # action verb. Only this exact attested native summary may use it.
            index = 0
            ACTIVATION["method"] = "sole-summary-default-action"
        if index is None:
            raise ValueError("TARGET_CONTROL_ACTION_REJECTED")
        ACTIVATION.update(stage="do-action", attempted=True)
        accepted = bool(atspi.Action.do_action(actions, index))
        ACTIVATION["accepted"] = accepted
        if not accepted:
            raise ValueError("TARGET_CONTROL_ACTION_REJECTED")
        if disclosure:
            ACTIVATION["stage"] = "verify-expanded"
            deadline = time.monotonic() + 1.2
            while time.monotonic() < deadline:
                pump_events(.15)
                node["state"] = fresh_state(node["object"], atspi)
                ACTIVATION["expandedAfter"] = node["state"].get("expanded") is True
                if ACTIVATION["expandedAfter"] == expected_expanded and is_showing(node):
                    ACTIVATION["stateVerified"] = True
                    break
            if not ACTIVATION["stateVerified"]:
                raise ValueError("ALBUM_DISCLOSURE_STATE_MISMATCH" if album_disclosure else "PROFILE_DISCLOSURE_NOT_EXPANDED")
        ACTIVATION["stage"] = "verified"
    return {"accepted": True, "name": node["name"], "role": node["role"], "bounds": node["bounds"],
            "activation": dict(ACTIVATION)}


def album_destination_snapshot(nodes):
    if not ALBUM_CONTEXT:
        raise ValueError("ALBUM_FIXTURE_CONTEXT_INVALID")
    criteria = {
        "heading": lambda n: n.get("albumHeadingAttested") and n["name"] == ALBUM_CONTEXT["title"],
        "downloadSummary": lambda n: n.get("summaryAttested") and n["name"] == ALBUM_CONTEXT["download"],
        "moreSummary": lambda n: n.get("summaryAttested") and n["name"] == ALBUM_CONTEXT["more"],
    }
    evidence = {}
    for key, matches in criteria.items():
        found = [node for node in nodes if matches(node)]
        evidence[key] = {"count": min(len(found), 8), "showing": len(found) == 1 and is_showing(found[0])}
    evidence["verified"] = all(item["count"] == 1 and item["showing"] for item in evidence.values())
    ALBUM_DESTINATION.clear(); ALBUM_DESTINATION.update(evidence)
    return {"kind": "album-detail", **evidence}


def observe_after_click(action, app, atspi, exact, contains, roles, prefix=None, postcondition=None):
    if postcondition == "album-detail":
        # Navigation intentionally unmounts the source card. Require the bound
        # destination heading and both exact attested album disclosures instead.
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            pump_events(.15)
            destination = album_destination_snapshot(collect(app, atspi))
            if destination["verified"]:
                action["destination"] = destination
                return
        raise ValueError("ALBUM_DESTINATION_NOT_VERIFIED")
    time.sleep(.35)
    after = locate(collect(app, atspi), exact, contains, roles, visible=True, prefix=prefix)
    action["after"] = {"name": after["name"], "role": after["role"],
                       "bounds": after["bounds"], "state": after["state"]}


def validate_album_operation(args):
    if args.postcondition == "album-detail":
        expected = [ALBUM_CONTEXT["title"], ALBUM_CONTEXT["artist"], ALBUM_CONTEXT["sourceProvider"]]
        if (args.action != "click" or args.name is not None or args.prefix is not None
                or args.contains != expected or set(args.roles) != {"button", "push button"}
                or args.toggle_summary is not None):
            raise ValueError("ALBUM_NAVIGATION_SELECTOR_INVALID")
    if args.toggle_summary is not None:
        if (args.action != "click" or args.contains or args.prefix is not None
                or summary_kind(args.name) not in {"album-download", "album-more"}
                or set(args.roles) != {"summary"} or args.postcondition is not None):
            raise ValueError("ALBUM_DISCLOSURE_MODE_INVALID")


def main():
    global ALBUM_CONTEXT
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", type=Path, required=True)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--dbus", required=True)
    parser.add_argument("--xdg", type=Path, required=True)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--name")
    group.add_argument("--contains", nargs="+", metavar="TEXT")
    group.add_argument("--prefix")
    parser.add_argument("--roles", nargs="+", default=["push button", "button", "toggle button", "page tab", "tab", "link"])
    parser.add_argument("--action", choices=["inspect", "click", "inspect-optional", "list"], default="click")
    parser.add_argument("--postcondition", choices=["album-detail"])
    parser.add_argument("--toggle-summary", choices=["open", "closed"])
    args = parser.parse_args()
    ACTIVATION.clear()
    DISCLOSURE_CANDIDATES.clear()
    ALBUM_CONTEXT = None
    ALBUM_DESTINATION.clear()
    try:
        display = checked_context(args.profile, args.pid, args.dbus, args.xdg)
        if args.postcondition or args.toggle_summary is not None:
            ALBUM_CONTEXT = load_album_context(args.profile)
            validate_album_operation(args)
        import gi
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi
        Atspi.init()
        app = app_accessible(args.pid, Atspi)
        nodes = collect(app, Atspi)
        if args.action in {"inspect-optional", "list"}:
            found = [n for n in nodes if n["role"] in set(args.roles)
                     and match_name(n, args.name, args.contains or [], args.prefix)
                     and (args.action == "inspect-optional" or is_showing(n))]
            print(json.dumps({"status": "library_album_atspi_action", "action": args.action,
                "appPid": args.pid, "profile": str(args.profile), "display": display,
                "found": bool(found), "count": len(found),
                "controls": [{"name": item["name"], "role": item["role"], "bounds": item["bounds"]}
                             for item in found[:12]]}, sort_keys=True))
            return 0
        node = locate(nodes, args.name, args.contains or [], set(args.roles), visible=False, prefix=args.prefix)
        action = {"accepted": False, "name": node["name"], "role": node["role"],
                  "bounds": node["bounds"], "state": node["state"]}
        if args.action == "click":
            action = click(node, Atspi, app, args.name, args.contains or [], set(args.roles), args.prefix, args.toggle_summary)
            observe_after_click(action, app, Atspi, args.name, args.contains or [], set(args.roles), args.prefix, args.postcondition)
        print(json.dumps({"status": "library_album_atspi_action", "action": args.action,
                          "appPid": args.pid, "profile": str(args.profile), "display": display, **action}, sort_keys=True))
        return 0
    except Exception as error:
        message = str(error)
        reason = message if re.fullmatch(r"[A-Z0-9_]{3,64}", message) else "LIBRARY_ATSPI_ACTION_FAILED"
        print(json.dumps({"status": "rejected", "action": args.action, "reasonCode": reason,
            "errorKind": error_kind(error), "diagnostics": {"disclosureCandidates": DISCLOSURE_CANDIDATES,
            "activation": ACTIVATION, "albumDestination": ALBUM_DESTINATION}}, sort_keys=True), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
