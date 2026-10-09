#!/usr/bin/env python3
"""AT-SPI assertions of the Dockering Linux smoke test (REL-074).

Plan: docs/plan/features/linux-install-smoke-test.md. GPUI publishes its accessibility tree over AT-SPI on
X11 and on Wayland, so the test asks the running app which page is selected and which named controls it
shows, without pixel coordinates or fonts.

  atspi.py tree                  the app is on the accessibility bus (waits up to 20 s)
  atspi.py page NAME             sidebar item NAME is selected and its page controls are present
  atspi.py settings              the Settings navigation is showing (General selected)
  atspi.py has ROLE NAME         a control with that role and accessible name exists (ROLE: entry, tab, button)
  atspi.py walk                  activate each sidebar item through the accessibility API and check its page

Needs a session bus with the AT-SPI bus enabled (run.sh starts both) and python3-gi with the Atspi
typelib. Prints one `name PASS|FAIL detail` line per check, like run.sh, and exits 1 on a FAIL.
SMOKE_OUT + SMOKE_SHOT (a screenshot command such as "import -window root") make `walk` keep a
screenshot per page. Only controls with an accessible name can be asserted: the rows and the icon
buttons of the tables have none, so what a list contains is checked on the screenshot, not here.
"""
import os
import shlex
import subprocess
import sys
import time
import warnings

warnings.filterwarnings("ignore")
import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

# Roles are compared by enum: libatspi 2.52 names role 43 "push button", newer builds "button".
R = Atspi.Role
ROLE = {"entry": R.ENTRY, "tab": R.PAGE_TAB, "button": R.PUSH_BUTTON, "item": R.TREE_ITEM}

# Named controls per page of the --demo profile; a rename in crates/dockering/src must be mirrored here.
PAGES = {
    "Containers": [(R.PAGE_TAB, "All"), (R.PUSH_BUTTON, "Group by: Compose project"), (R.ENTRY, "Search…")],
    "Images": [(R.PAGE_TAB, "All"), (R.PUSH_BUTTON, "Pull"), (R.ENTRY, "Search…")],
    "Volumes": [(R.PAGE_TAB, "All"), (R.PUSH_BUTTON, "Create"), (R.ENTRY, "Search…")],
    "Networks": [(R.ENTRY, "Search…")],
}
ORDER = ["Images", "Volumes", "Networks", "Containers"]
failed = 0


def check(name, ok, detail=""):
    global failed
    failed += 0 if ok else 1
    print(f"{name:<22} {'PASS' if ok else 'FAIL':<5} {detail}", flush=True)
    return ok


def application():
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        node = desktop.get_child_at_index(i)
        if node is not None and node.get_name() == "dockering":
            return node
    return None


def walk(node):
    yield node
    for i in range(node.get_child_count()):
        try:
            child = node.get_child_at_index(i)
        except Exception:
            continue
        if child is not None:
            yield from walk(child)


def named(role):
    app = application()
    return [n for n in walk(app) if n.get_role() == role and n.get_name()] if app else []


def selected(node):
    states = node.get_state_set()
    return states.contains(Atspi.StateType.SELECTED) or states.contains(Atspi.StateType.CHECKED)


def wait_for(pred, timeout=None):
    end = time.time() + (timeout if timeout is not None else float(os.environ.get("ATSPI_TIMEOUT", "8")))
    while time.time() < end:
        try:
            value = pred()
        except Exception:
            value = None
        if value:
            return value
        time.sleep(0.3)
    return None


def sidebar_item(name):
    return next((n for n in named(R.TREE_ITEM) if n.get_name() == name), None)


def back_item():
    # on a Settings route the first sidebar item is "Back to <page>" (strings::back_to)
    return next((n for n in named(R.TREE_ITEM) if n.get_name().startswith("Back to ")), None)


def page_is(name):
    item = sidebar_item(name)
    return item is not None and selected(item)


def missing_controls(page):
    expected = PAGES.get(page)
    if expected is None:
        return [f"no expectations for page {page!r}"]
    return [f"{Atspi.role_get_name(r)} {n!r}" for r, n in expected if n not in {x.get_name() for x in named(r)}]


def settled_missing(page):
    wait_for(lambda: not missing_controls(page), 5)
    return missing_controls(page)


def shot(label):
    cmd, out = os.environ.get("SMOKE_SHOT"), os.environ.get("SMOKE_OUT")
    if cmd and out:
        subprocess.run(shlex.split(cmd) + [os.path.join(out, f"atspi-{label}.png")], check=False,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def cmd_tree():
    ok = wait_for(lambda: sidebar_item("Containers") or back_item())
    check("atspi_tree", bool(ok), f"{len(list(walk(application())))} nodes" if ok else "the app never appeared on the accessibility bus")


def cmd_page(name, label=None):
    ok = wait_for(lambda: page_is(name))
    missing = settled_missing(name)
    check(label or f"page_{name}", bool(ok) and not missing,
          f"{name} selected, controls present" if ok and not missing else f"selected={bool(ok)} missing={missing}")


def cmd_settings(label="page_Settings"):
    ok = wait_for(lambda: back_item() and sidebar_item("General") and selected(sidebar_item("General")))
    check(label, bool(ok), "Settings navigation shown, General selected" if ok else "Settings navigation not found")


def cmd_has(role, name):
    ok = wait_for(lambda: any(n.get_name() == name for n in named(ROLE[role])))
    check(f"has_{role}", bool(ok), repr(name))


def cmd_walk():
    for page in ORDER:
        item = sidebar_item(page)
        acted = item.get_action_iface().do_action(0) if item is not None else False
        ok = wait_for(lambda: page_is(page))
        time.sleep(0.8)  # let the page paint before the screenshot
        shot(page.lower())
        missing = settled_missing(page)
        check(f"click_{page}", bool(acted and ok) and not missing,
              "activated, selected, controls present" if acted and ok and not missing
              else f"action={acted} selected={bool(ok)} missing={missing}")


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    cmd, args = argv[0], argv[1:]
    if cmd == "tree":
        cmd_tree()
    elif cmd == "page":
        cmd_page(args[0], f"page_{args[0]}")
    elif cmd == "settings":
        cmd_settings()
    elif cmd == "has":
        cmd_has(args[0], " ".join(args[1:]))
    elif cmd == "walk":
        cmd_walk()
    else:
        print(__doc__)
        return 2
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
