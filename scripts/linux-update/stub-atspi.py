#!/usr/bin/env python3
"""Stand-in for atspi_update.py in the runner's self-test (selftest.sh; E2E_STUB=1 makes s1.sh call this instead).

The stub app (stub_app.py) has no accessibility tree; it writes what its window would show to ~/.stub-ui.json. This answers the
subcommands of atspi_update.py from that file and prints the same `name PASS|FAIL|INFO detail` lines, so s1.sh's own logic (its
ordering, its process, file and request checks, its SKIP rules) is what the self-test exercises. The AT-SPI code itself is the
real run's job and has its own unit tests (scripts/tests/test_linux_update.py).
"""
import argparse
import json
import os
import sys
import time

UI = os.path.join(os.environ["HOME"], ".stub-ui.json")
CLICK = os.path.join(os.environ["HOME"], ".stub-click")


def say(name, status, detail=""):
    print(f"{name:<22} {status:<5} {detail}", flush=True)
    return status != "FAIL"


def read_ui():
    """What the app's window shows now, or None while no live app has written it."""
    try:
        with open(UI, encoding="utf-8") as handle:
            ui = json.load(handle)
        os.kill(ui["pid"], 0)
        return ui
    except (OSError, ValueError, KeyError):
        return None


def wait_for(pred, timeout):
    end = time.time() + timeout
    while True:
        value = pred()
        if value or time.time() >= end:
            return value
        time.sleep(0.1)


def main(argv):
    parser = argparse.ArgumentParser(prog="stub-atspi.py")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("tree")
    p.add_argument("--name", default="window")
    p.add_argument("--timeout", type=float, default=45)
    p = sub.add_parser("absent")
    p.add_argument("version")
    p.add_argument("--name", required=True)
    p.add_argument("--for", dest="hold", type=float, default=0.0)
    p = sub.add_parser("watch")
    p.add_argument("version")
    p.add_argument("--timeout", type=float, default=45)
    p = sub.add_parser("activate")
    p.add_argument("version")
    p = sub.add_parser("notice")
    p.add_argument("version")
    p.add_argument("--timeout", type=float, default=45)
    sub.add_parser("dump")
    args = parser.parse_args(argv)

    if args.command == "tree":
        ui = wait_for(read_ui, args.timeout)
        return say(args.name, "PASS" if ui else "FAIL", "the stub's window is up" if ui else "the stub never wrote its window")
    if args.command == "absent":
        end = time.time() + args.hold
        while True:
            ui = read_ui()
            if ui is None:
                return say(args.name, "FAIL", "no accessibility tree to look in: an absence can't be shown")
            if ui.get("button"):
                return say(args.name, "FAIL", f"an update control is shown: {ui['button']!r}")
            if time.time() >= end:
                return say(args.name, "PASS", "no 'Update available' control")
            time.sleep(0.2)
    if args.command == "watch":
        name = f"Update available: Dockering {args.version}"
        ui = wait_for(lambda: (lambda u: u if u and u.get("button") == name else None)(read_ui()), args.timeout)
        if ui:
            say("button", "PASS", f"{name!r} (role button)")
            say("toast_atspi", "INFO", "the stub shows no toast")
            return True
        return say("button", "FAIL", f"no {name!r} button in the stub's window")
    if args.command == "activate":
        ui = read_ui()
        if not ui or not ui.get("button"):
            return say("activate", "FAIL", "the update button is gone before it could be clicked")
        open(CLICK, "w").close()
        return say("activate", "PASS", "do_action(0) on the update button (the stub's click file)")
    if args.command == "notice":
        ui = wait_for(lambda: (lambda u: u if u and u.get("notice") else None)(read_ui()), args.timeout)
        if ui:
            return say("notice", "PASS", "'What's new' button shown")
        return say("notice", "FAIL", "no 'What's new' button: the post-update notice did not appear")
    print(json.dumps(read_ui()))
    return True


if __name__ == "__main__":
    sys.exit(0 if main(sys.argv[1:]) else 1)
