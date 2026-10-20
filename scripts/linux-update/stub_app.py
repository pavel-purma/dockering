#!/usr/bin/env python3
"""The behaviour of the Dockering AppImage test double of the runner's self-test (stub-app.sh starts it; selftest.sh drives it).

    stub_app.py <old|new> <version> [args...]

Role `old` is the installed app: it takes the single-instance socket, makes the update check on the seam's schedule (the three
requests of S1, with the User-Agent of a real 0.0.1 build), keeps the retained offer in ~/.local/share/dockering/updates/offer.json,
"shows" the title-bar button by writing ~/.stub-ui.json, which stub-atspi.py reads in place of the accessibility tree, and when
stub-atspi.py's `activate` creates ~/.stub-click it stages the downloaded file next to itself, self-tests it with `--version` and
swaps it over ~/.local/bin/dockering, saves its window bounds, starts the new file with `--update-relaunch=1` and a piped
stdin, and exits (the pipe closes with it). Role `new` is what gets swapped in: it waits for end-of-file on stdin (UPD-017), takes
the socket, restores the state, and "shows" the post-update notice.

Like the product it decides by what it finds, not by being told the scenario: an install directory it can't write to gets a
Download offer and no payload (S3); a staged file whose `--version` is not `dockering <manifest version>` fails the self-test, so
the offer degrades to Download and nothing is replaced (S2); DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS delays the exit (S4).

E2E_STUB_FLAGS (space separated) injects one defect each, so that selftest.sh can prove that exactly the intended checks fail:
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
  swap_anyway   old (S2): skips the self-test and installs the file that fails it            -> the S2 checks of the file and the offer
  exits_on_failure old (S2): exits after the failed install                                  -> old_alive, button_after_failure
  no_degrade    old (S2): keeps a Ready offer after the failed install                       -> offer_degraded
  stage_left    old (S2): leaves its staged copy behind                                      -> no_staging_left
  redownload    old (S2): downloads the payload again after the failure                      -> no_second_check
  asset_early   old (S3): fetches the payload although the install is read-only              -> requests, no_second_check
  asset_late    old (S3): fetches it a little later                                          -> no_second_check
  offer_stuck   old (S3): leaves the Download offer in the downloading stage                 -> offer_download
  exits_late    old (S3): exits a little after the announcement                              -> old_alive
  no_delay      old (S4): ignores the seam's exit delay                                      -> exit_delayed, handshake
  pipe_early    old (S4): closes the relaunch pipe at once, as a destructor would            -> handshake
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
OFFER = f"{DATA}/updates/offer.json"
SOCKET = f"{DATA}/instance.sock"
LOG = f"{DATA}/logs/dockering.stub.log"
UI = f"{HOME}/.stub-ui.json"
CLICK = f"{HOME}/.stub-click"
BINDIR = f"{HOME}/.local/bin"
TARGET = f"{BINDIR}/dockering"
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
        with open(UI, encoding="utf-8") as handle:
            mine = json.load(handle).get("pid") == os.getpid()
        if mine:  # a new process that came up while this one lingers owns the window now
            os.unlink(UI)
    except (OSError, ValueError):
        pass
    os._exit(code)


def older(left, right):
    key = lambda text: tuple(int(part) for part in text.split("-")[0].split("."))  # noqa: E731
    return key(left) < key(right)


# ---- role old ---------------------------------------------------------------------------------------------------------

def install_dir_writable():
    """What the product's install-kind probe asks: can a staging file be created next to the installed AppImage?"""
    probe = f"{BINDIR}/.dockering-update-probe-{os.getpid()}"
    try:
        open(probe, "x", encoding="utf-8").close()
    except OSError:
        return False
    os.unlink(probe)
    return True


def fetch_payload(version):
    payload = f"{DATA}/updates/{version}/Dockering-x86_64.AppImage"
    os.makedirs(os.path.dirname(payload), exist_ok=True)
    with open(payload, "wb") as handle:
        handle.write(get(f"{REPO}/download/v{version}/Dockering-x86_64.AppImage"))
    return payload


def write_offer(manifest, stage, failed):
    """~/.local/share/dockering/updates/offer.json as crates/dk-update/src/offer.rs writes it."""
    os.makedirs(os.path.dirname(OFFER), exist_ok=True)
    write_json(OFFER, {"version": 1, "manifest": manifest, "signature": "stub signature", "confirmed_at": stamp(),
                       "stage": stage, "install_failed": failed})


def check_for_update():
    """The requests of the first check. An install that can't update itself (a read-only directory, S3) gets a Download offer
    and nothing is fetched; any other gets the payload."""
    manifest = get(MANIFEST)
    get(MANIFEST + ".minisig")
    version = json.loads(manifest)["version"]
    download_only = not install_dir_writable()
    payload = None
    if not download_only or "asset_early" in FLAGS:
        payload = fetch_payload(version)
    return {"version": version, "manifest": manifest.decode("utf-8"), "payload": payload, "download_only": download_only,
            "at": time.time(), "late_done": False}


def announce(done):
    version = done["version"]
    state = load_state()
    state.setdefault("updates", {})["notified_version"] = version
    save_state(state)
    # the retained offer: a verified install offer is Ready; a Download offer is as actionable as it will ever be
    write_offer(done["manifest"], "downloading" if "offer_stuck" in FLAGS else "ready", False)
    if "nobutton" not in FLAGS:
        ui(button=f"Update available: Dockering {version}", toast="download" if done["download_only"] else "restart")
    log("INFO", f'update ready version="{version}"')


def self_test(staged, version, env):
    """UPD-014 (3): the staged file must print `dockering <manifest version>` for --version and exit 0."""
    try:
        run = subprocess.run([staged, "--version"], stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=20, env=env, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return False
    return run.returncode == 0 and run.stdout.strip() == f"dockering {version}"


def fail_install(done, staged):
    """UPD-017 (6): a failed self-test removes the staged file and the payload and degrades the offer to Download for good."""
    if "stage_left" not in FLAGS:
        os.unlink(staged)
    log("WARN", f'update install failed version="{done["version"]}": the staged file failed its --version self-test')
    if "no_degrade" not in FLAGS:
        write_offer(done["manifest"], "ready", True)
        os.unlink(done["payload"])
    ui(failure="Couldn't start the update: the new version failed its self-test")
    if "exits_on_failure" in FLAGS:
        leave(1)
    if "redownload" in FLAGS:
        get(f"{REPO}/download/v{done['version']}/Dockering-x86_64.AppImage")  # asked for again; the bytes are not kept


def apply_update(done):
    stage = f"{BINDIR}/.dockering-update-{os.getpid()}"
    shutil.copyfile(done["payload"], stage)
    os.chmod(stage, 0o755)
    env = {k: v for k, v in os.environ.items() if k not in ("APPIMAGE", "APPDIR", "ARGV0", "OWD", "E2E_STUB_APPIMAGE")}
    if "swap_anyway" not in FLAGS and not self_test(stage, done["version"], env):
        fail_install(done, stage)
        return
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
    # run() has returned: the socket is released. The seam then delays the exit (S4), and the pipe closes with the process.
    let_go()
    if "pipe_early" in FLAGS:
        child.stdin.close()  # what a destructor that drops the pipe's write end would do
    delay = 0 if "no_delay" in FLAGS else int(os.environ.get("DOCKERING_UPDATE_E2E_QUIT_DELAY_SECS") or 0)
    if delay:
        time.sleep(delay)
    leave(0)


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
            announce(done)
        if done is not None:
            if "asset_late" in FLAGS and not done["late_done"] and time.time() >= done["at"] + 2.5:
                done["late_done"] = True
                fetch_payload(done["version"])
            if "exits_late" in FLAGS and time.time() >= done["at"] + 2.0:
                leave(0)
        if done is not None and os.path.exists(CLICK):
            os.unlink(CLICK)
            if os.environ.get("E2E_STUB_RESTART_DELAY"):
                time.sleep(float(os.environ["E2E_STUB_RESTART_DELAY"]))
            apply_update(done)
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
