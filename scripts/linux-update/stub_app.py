#!/usr/bin/env python3
"""The behaviour of the Dockering AppImage test double of the runner's self-test (stub-app.sh starts it; selftest.sh drives it).

    stub_app.py <old|new> <version> [args...]

Role `old` is the installed app: it takes the single-instance socket, makes the update check on the seam's schedule (the three
requests of S1, with the User-Agent of a real 0.0.1 build), "shows" the title-bar button by writing ~/.stub-ui.json, which
stub-atspi.py reads in place of the accessibility tree, and when stub-atspi.py's `activate` creates ~/.stub-click it swaps the
downloaded file over ~/.local/bin/dockering, saves its window bounds, starts the new file with `--update-relaunch=1` and a piped
stdin, and exits (the pipe closes with it). Role `new` is what gets swapped in: it waits for end-of-file on stdin (UPD-017), takes
the socket, restores the state, and "shows" the post-update notice.

E2E_STUB_FLAGS (space separated) injects one defect each, so that selftest.sh can prove that exactly one check of s1.sh fails:
  noswap        old: never replaces the file; starts the downloaded copy instead              -> swap
  nobutton      old: never shows the button (it still announces the update in state.json)    -> button
  early_button  old: shows a button before the check is due                                  -> before_button
  early_request old: makes the check at once, before it is due                               -> before_requests
  wrong_ua      old: sends another User-Agent                                                -> user_agent
  exit_early    old: swaps the file, then exits without starting the new one                 -> relaunch
  old_lingers   old: starts the new one, lets go of the socket and the pipe, then lingers    -> old_exit
  no_handshake  new: logs a timed-out relaunch wait instead of end-of-file                   -> handshake
  not_primary   new: never takes the single-instance socket                                  -> primary_instance
  stale_state   new: writes state.json without the old instance's window bounds              -> state_kept
  second_check  new: asks for the manifest again                                             -> no_second_check
  no_notice     new: shows no post-update notice                                             -> notice
  button_again  new: shows the update button again                                           -> no_button_after
"""
import json
import os
import select
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.request

HOME = os.environ["HOME"]
DATA = f"{HOME}/.local/share/dockering"
STATE = f"{DATA}/state.json"
SOCKET = f"{DATA}/instance.sock"
LOG = f"{DATA}/logs/dockering.stub.log"
UI = f"{HOME}/.stub-ui.json"
CLICK = f"{HOME}/.stub-click"
TARGET = f"{HOME}/.local/bin/dockering"
REPO = "/pavel-purma/dockering/releases"
MANIFEST = f"{REPO}/latest/download/dockering-update.json"
UA = "Dockering/0.0.1 (linux; x86_64)"
FLAGS = set(os.environ.get("E2E_STUB_FLAGS", "").split())
SOCK = None


def stamp():
    return time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()) + ".000000Z"


def log(level, text):
    os.makedirs(os.path.dirname(LOG), exist_ok=True)
    with open(LOG, "a", encoding="utf-8") as handle:
        handle.write(f"{stamp()} {level:>5} {text}\n")


def write_json(path, value):
    tmp = f"{path}.{os.getpid()}.tmp"
    with open(tmp, "w", encoding="utf-8") as handle:
        json.dump(value, handle)
    os.replace(tmp, path)


def load_state():
    try:
        with open(STATE, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return {"version": 1, "window": None, "updates": {}}


def save_state(state):
    os.makedirs(DATA, exist_ok=True)
    write_json(STATE, state)


def ui(**values):
    try:
        with open(UI, encoding="utf-8") as handle:
            current = json.load(handle)
    except (OSError, ValueError):
        current = {}
    current.update(values, pid=os.getpid(), ready=True)
    write_json(UI, current)


def get(path):
    request = urllib.request.Request(os.environ["DOCKERING_UPDATE_E2E_URL"] + path, headers={"User-Agent": UA if "wrong_ua" not in FLAGS else "curl/8.9 (stub)"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read()


def take_socket():
    global SOCK
    os.makedirs(DATA, exist_ok=True)
    SOCK = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        os.unlink(SOCKET)
    except FileNotFoundError:
        pass
    SOCK.bind(SOCKET)
    SOCK.listen(4)


def let_go():
    global SOCK
    if SOCK is not None:
        SOCK.close()
        SOCK = None
        try:
            os.unlink(SOCKET)
        except FileNotFoundError:
            pass


def leave(code=0):
    let_go()
    try:
        os.unlink(UI)
    except FileNotFoundError:
        pass
    os._exit(code)


def older(left, right):
    key = lambda text: tuple(int(part) for part in text.split("-")[0].split("."))  # noqa: E731
    return key(left) < key(right)


# ---- role old ---------------------------------------------------------------------------------------------------------

def check_for_update():
    """The three requests of S1; returns (version, payload path)."""
    manifest = get(MANIFEST)
    get(MANIFEST + ".minisig")
    version = json.loads(manifest)["version"]
    payload = f"{DATA}/updates/{version}/Dockering-x86_64.AppImage"
    os.makedirs(os.path.dirname(payload), exist_ok=True)
    with open(payload, "wb") as handle:
        handle.write(get(f"{REPO}/download/v{version}/Dockering-x86_64.AppImage"))
    return version, payload


def announce(version):
    state = load_state()
    state.setdefault("updates", {})["notified_version"] = version
    save_state(state)
    if "nobutton" not in FLAGS:
        ui(button=f"Update available: Dockering {version}", toast=True)
    log("INFO", f'update ready version="{version}"')


def apply_update(payload):
    stage = f"{HOME}/.local/bin/.dockering-update-{os.getpid()}"
    shutil.copyfile(payload, stage)
    os.chmod(stage, 0o755)
    env = {k: v for k, v in os.environ.items() if k not in ("APPIMAGE", "APPDIR", "ARGV0", "OWD", "E2E_STUB_APPIMAGE")}
    if "noswap" in FLAGS:
        start = stage
        env["E2E_STUB_APPIMAGE"] = TARGET  # the AppImage runtime would say so; the file on disk is not replaced
    else:
        os.rename(stage, TARGET)
        start = TARGET
    state = load_state()
    state["window"] = {"x": 120.0, "y": 80.0, "width": 1100.0, "height": 720.0, "maximized": False}
    save_state(state)
    if "exit_early" in FLAGS:
        leave(0)
    child = subprocess.Popen([start, "--update-relaunch=1"], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, start_new_session=True, env=env, close_fds=True)
    if "old_lingers" in FLAGS:
        let_go()
        child.stdin.close()
        time.sleep(25)
    leave(0)  # the socket first, then the process: the pipe's write end closes with it


def run_old(version):
    take_socket()
    log("INFO", f'starting version="{version}" demo=false')
    state = load_state()
    state.setdefault("updates", {})["last_run_version"] = version
    save_state(state)
    ui(button=None, toast=False, notice=False)
    if "early_button" in FLAGS:
        ui(button="Update available: Dockering 9.9.9")
    signal.signal(signal.SIGTERM, lambda *_: leave(0))
    first = int(os.environ.get("DOCKERING_UPDATE_E2E_FIRST_CHECK_SECS", "30"))
    due = time.time() + (0 if "early_request" in FLAGS else first)
    done = None
    while True:
        if done is None and time.time() >= due:
            done = check_for_update()
            announce(done[0])
        if done is not None and os.path.exists(CLICK):
            os.unlink(CLICK)
            if os.environ.get("E2E_STUB_RESTART_DELAY"):
                time.sleep(float(os.environ["E2E_STUB_RESTART_DELAY"]))
            apply_update(done[1])
        time.sleep(0.1)


# ---- role new ---------------------------------------------------------------------------------------------------------

def wait_for_predecessor(limit):
    """UPD-017: block until stdin reaches end-of-file; returns (outcome, milliseconds)."""
    started = time.time()
    while time.time() - started < limit:
        ready, _, _ = select.select([0], [], [], 0.1)
        if ready and not os.read(0, 4096):
            return "Eof", int((time.time() - started) * 1000)
    return "TimedOut", int((time.time() - started) * 1000)


def run_new(version, args):
    waited = None
    if "--update-relaunch=1" in args and not sys.stdin.isatty():
        waited = wait_for_predecessor(30)
    state = load_state()
    previous = (state.get("updates") or {}).get("last_run_version")
    if waited is not None:
        if "no_handshake" in FLAGS:
            log("WARN", "update relaunch: the previous instance did not exit in time; continuing as a normal launch")
        else:
            log("INFO", f"update relaunch: wait finished waited_ms={waited[1]} outcome={waited[0]}")
    log("INFO", f'starting version="{version}" demo=false')
    try:
        probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        probe.connect(SOCKET)
        log("INFO", "another instance is running; asked it to focus")
        sys.exit(0)
    except OSError:
        pass
    if "not_primary" not in FLAGS:
        take_socket()
    if "stale_state" in FLAGS:
        state["window"] = None
    state.setdefault("updates", {})["last_run_version"] = version
    save_state(state)
    notice = previous is not None and older(previous, version) and "no_notice" not in FLAGS
    ui(button="Update available: Dockering 9.9.9" if "button_again" in FLAGS else None, toast=False, notice=notice)
    if "second_check" in FLAGS:
        get(MANIFEST)
    signal.signal(signal.SIGTERM, lambda *_: leave(0))
    while True:
        time.sleep(0.2)


def main():
    role, version, args = sys.argv[1], sys.argv[2], sys.argv[3:]
    if "--version" in args:
        print(f"dockering {version}")
        return 0
    if role == "old":
        run_old(version)
    else:
        run_new(version, args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
