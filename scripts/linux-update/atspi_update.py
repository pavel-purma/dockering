#!/usr/bin/env python3
"""AT-SPI assertions of the Linux update end-to-end test (REL-085, UPD-015; plan linux-appimage-updates §6.5).

Like scripts/linux-smoke/atspi.py (whose helpers are copied, not imported: that harness is release-gating and stays
untouched), for the update scenarios. GPUI publishes its accessibility tree over AT-SPI; only controls with an accessible
name can be asserted. Prints one `name PASS|FAIL|INFO detail` line per check and exits 1 on a FAIL.

  atspi_update.py tree [--name NAME] [--timeout S]            the app is on the accessibility bus and has painted its tree
  atspi_update.py absent VERSION --name NAME [--for S]        no 'Update available...' control (any role) for S seconds
  atspi_update.py watch VERSION [--timeout S]                 the title-bar button, role button, named exactly
                                                              'Update available: Dockering VERSION' (UPD-015); and, only as
                                                              INFO (a toast hides itself), whether the toast was seen
  atspi_update.py activate VERSION                            do_action(0) on that button: the click, RestartToUpdate
  atspi_update.py notice VERSION [--timeout S]                the post-update notice is shown: its 'What's new' button (UPD-008)
  atspi_update.py dump                                        one line per node: depth, role, name (evidence, not a check)

Needs a session bus with the AT-SPI bus enabled (s1.sh starts both) and python3-gi with the Atspi typelib. A crash prints a FAIL.
"""
import argparse
import os
import sys
import time
import warnings

warnings.filterwarnings("ignore")

BUTTON_PREFIX = "Update available"
TOAST_BUTTON = "Restart now"
NOTICE_BUTTON = "What's new"
MIN_NODES = 10  # a window that has painted, not just a registered application


def button_name(version):
    return f"{BUTTON_PREFIX}: Dockering {version}"


def classify(nodes, version, push_button):
    """What the update scenarios look for in one snapshot of the tree: [(role, name, node)] -> a dict.

    `button`: the node of the title-bar button (role `push_button`, exact name); `lookalikes`: other controls whose name
    starts with 'Update available' (a wrong role or a wrong version is a finding, not an absence).
    """
    found = {"count": len(nodes), "button": None, "lookalikes": [], "restart_now": False, "whats_new": False, "texts": []}
    for role, name, node in nodes:
        if not name:
            continue
        if name.startswith(BUTTON_PREFIX):
            if role == push_button and name == button_name(version):
                found["button"] = node
            else:
                found["lookalikes"].append(name)
        elif role == push_button and name == TOAST_BUTTON:
            found["restart_now"] = True
        elif role == push_button and name == NOTICE_BUTTON:
            found["whats_new"] = True
        elif "ready to install" in name or "Updated to Dockering" in name:
            found["texts"].append(name)
    return found


def say(name, status, detail=""):
    print(f"{name:<22} {status:<5} {detail}", flush=True)
    return status != "FAIL"


def connect():
    """The Atspi module; imported late so that classify() can be tested without GObject."""
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    return Atspi


class Tree:
    def __init__(self):
        self.atspi = connect()
        self.push_button = self.atspi.Role.PUSH_BUTTON  # roles are compared by enum: libatspi names role 43 differently by version

    def application(self):
        desktop = self.atspi.get_desktop(0)
        for i in range(desktop.get_child_count()):
            node = desktop.get_child_at_index(i)
            if node is not None and node.get_name() == "dockering":
                return node
        return None

    def walk(self, node, depth=0):
        yield depth, node
        for i in range(node.get_child_count()):
            try:
                child = node.get_child_at_index(i)
            except Exception:
                continue
            if child is not None:
                yield from self.walk(child, depth + 1)

    def snapshot(self):
        """[(role, name, node)] of the whole app, or None while it is not on the bus."""
        app = self.application()
        if app is None:
            return None
        out = []
        for _, node in self.walk(app):
            try:
                out.append((node.get_role(), node.get_name() or "", node))
            except Exception:
                continue
        return out

    def view(self, version):
        nodes = self.snapshot()
        return None if nodes is None else classify(nodes, version, self.push_button)


def wait_for(pred, timeout):
    end = time.time() + timeout
    while True:
        try:
            value = pred()
        except Exception:
            value = None
        if value:
            return value
        if time.time() >= end:
            return None
        time.sleep(0.3)


def timeout_of(args, default):
    return args.timeout if args.timeout is not None else float(os.environ.get("ATSPI_TIMEOUT", default))


def cmd_tree(tree, args):
    def painted():
        view = tree.view("")
        return view["count"] if view and view["count"] >= MIN_NODES else 0

    started = time.time()
    seen = wait_for(painted, timeout_of(args, 45))
    if seen:
        return say(args.name, "PASS", f"on the accessibility bus with {seen} nodes after {time.time() - started:.1f}s")
    return say(args.name, "FAIL", "the app never showed a painted accessibility tree (is it on the bus? did it crash?)")


def cmd_absent(tree, args):
    view = tree.view(args.version)
    if view is None or view["count"] < MIN_NODES:
        return say(args.name, "FAIL", "no accessibility tree to look in: an absence can't be shown")
    end = time.time() + args.hold
    while True:
        view = tree.view(args.version)
        if view is None:
            return say(args.name, "FAIL", "the app left the accessibility bus while the absence was observed")
        shown = view["lookalikes"] + (["the button"] if view["button"] is not None else [])
        if shown:
            return say(args.name, "FAIL", f"an update control is shown: {shown[0]!r}")
        if time.time() >= end:
            return say(args.name, "PASS", f"no 'Update available' control in {view['count']} nodes" + (f" for {args.hold:g}s" if args.hold else ""))
        time.sleep(0.4)


def cmd_watch(tree, args):
    seen = {"toast": False, "texts": []}

    def look():
        view = tree.view(args.version)
        if view is None:
            return None
        seen["toast"] = seen["toast"] or view["restart_now"]
        seen["texts"] = seen["texts"] or view["texts"]
        seen["view"] = view
        return view["button"] is not None and view

    started = time.time()
    view = wait_for(look, timeout_of(args, 45))
    if view:
        say("button", "PASS", f"{button_name(args.version)!r} (role button) after {time.time() - started:.1f}s")
        if seen["toast"]:
            say("toast_atspi", "PASS", f"the toast's '{TOAST_BUTTON}' button was in the tree: {seen['texts'][:1]}")
        else:
            say("toast_atspi", "INFO", "the toast was not in the tree (it hides itself; its announcement is asserted by toast)")
        return True
    last = seen.get("view") or {}
    if last.get("lookalikes"):
        return say("button", "FAIL", f"a control named {last['lookalikes'][0]!r} exists, expected {button_name(args.version)!r} with role button")
    return say("button", "FAIL", f"no {button_name(args.version)!r} button in the tree after {timeout_of(args, 45):g}s ({last.get('count', 0)} nodes)")


def cmd_activate(tree, args):
    view = tree.view(args.version)
    node = view and view["button"]
    if node is None:
        return say("activate", "FAIL", "the update button is gone before it could be clicked")
    actions = node.get_action_iface()
    acted = bool(actions and actions.do_action(0))
    return say("activate", "PASS" if acted else "FAIL", "do_action(0) on the update button" if acted else "the button has no action or refused it")


def cmd_notice(tree, args):
    def shown():
        view = tree.view(args.version)
        return view if view and view["whats_new"] else None

    started = time.time()
    view = wait_for(shown, timeout_of(args, 45))
    if view:
        text = view["texts"][0] if view["texts"] else "text not in the tree"
        return say("notice", "PASS", f"'{NOTICE_BUTTON}' button shown after {time.time() - started:.1f}s ({text})")
    return say("notice", "FAIL", f"no '{NOTICE_BUTTON}' button: the post-update notice did not appear")


def cmd_dump(tree, args):
    app = tree.application()
    if app is None:
        print("(the app is not on the accessibility bus)")
        return False
    for depth, node in tree.walk(app):
        try:
            print(f"{'  ' * depth}{tree.atspi.role_get_name(node.get_role())} {node.get_name()!r}")
        except Exception as error:
            print(f"{'  ' * depth}(unreadable: {type(error).__name__})")
    return True


def main(argv):
    parser = argparse.ArgumentParser(prog="atspi_update.py", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("tree")
    p.add_argument("--name", default="window")
    p.add_argument("--timeout", type=float)
    p = sub.add_parser("absent")
    p.add_argument("version")
    p.add_argument("--name", required=True)
    p.add_argument("--for", dest="hold", type=float, default=0.0)
    p = sub.add_parser("watch")
    p.add_argument("version")
    p.add_argument("--timeout", type=float)
    p = sub.add_parser("activate")
    p.add_argument("version")
    p = sub.add_parser("notice")
    p.add_argument("version")
    p.add_argument("--timeout", type=float)
    sub.add_parser("dump")
    args = parser.parse_args(argv)
    handlers = {"tree": cmd_tree, "absent": cmd_absent, "watch": cmd_watch, "activate": cmd_activate, "notice": cmd_notice, "dump": cmd_dump}
    try:
        return 0 if handlers[args.command](Tree(), args) else 1
    except Exception as error:
        say(f"atspi_{args.command}", "FAIL", f"crashed: {type(error).__name__}: {str(error)[:120]}")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
