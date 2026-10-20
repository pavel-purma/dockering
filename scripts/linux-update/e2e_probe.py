#!/usr/bin/env python3
"""Probes of the Linux update end-to-end test that need no accessibility tree (REL-085; plan linux-appimage-updates §6.5).

s1.sh runs these next to atspi_update.py. The `check` subcommands print one `name PASS|FAIL|INFO detail` line, the format of
scripts/linux-smoke/run.sh, and exit 1 on a FAIL; the others print plain values for the shell.

  e2e_probe.py family INSTALL [--exclude PID...] [--arg ARG] [-v]
      pids of the processes of the app installed at INSTALL (one per line): argv[0] is INSTALL, or the environment says
      APPIMAGE=INSTALL (the AppImage runtime sets it for the app it starts). `--version` runs are not the app. The old
      process, the AppImage runtime and the new process after a relaunch are all found this way, whatever their names.
  e2e_probe.py owner SOCKET
      the pid that listens on the unix socket SOCKET (the single-instance socket), or nothing
  e2e_probe.py requests LOG --version V [--arch A] [--name NAME]
      the server saw exactly one manifest, one .minisig and one asset request (all 200) and nothing else
  e2e_probe.py schedule LOG --start EPOCH --first-check SECS [--name NAME]
      the first request came no earlier than the first check was due (slack 1 s) and within 30 s after it
  e2e_probe.py user_agent LOG --expect UA [--name NAME]
      every request carried exactly this User-Agent
  e2e_probe.py state FILE --version V --want toast|kept [--name NAME]
      toast: state.json records the announcement (`updates.notified_version`); kept: the old instance's window bounds
      survived and the new instance recorded its version
  e2e_probe.py handshake LOG
      the relaunched process logged the end of its wait for the predecessor with outcome Eof (not a timeout)
"""
import argparse
import json
import os
import re
import sys
from datetime import datetime, timezone

REPO = "pavel-purma/dockering"  # xtask's UPDATER_REPO: the seam keeps GitHub's paths
MANIFEST_PATH = f"/{REPO}/releases/latest/download/dockering-update.json"
ANSI = re.compile(r"\x1b\[[0-9;]*m")
SCHEDULE_SLACK = 1.0  # the app counts from its own start, a little after the shell's
SCHEDULE_LATE = 30.0


def say(name, status, detail=""):
    print(f"{name:<22} {status:<5} {detail}", flush=True)
    return 0 if status != "FAIL" else 1


def read_log(path):
    """The request log: one JSON object per line. A missing file is an empty log; a torn line is an error."""
    if not os.path.exists(path):
        return []
    entries = []
    with open(path, encoding="utf-8") as handle:
        for number, line in enumerate(handle, 1):
            if line.strip():
                try:
                    entries.append(json.loads(line))
                except ValueError as error:
                    raise SystemExit(f"{path}:{number}: not JSON: {error}")
    return entries


def parse_ts(text):
    return datetime.strptime(text, "%Y-%m-%dT%H:%M:%S.%fZ").replace(tzinfo=timezone.utc).timestamp()


# ---- processes -----------------------------------------------------------------------------------------------------

def proc_text(pid, name):
    try:
        with open(f"/proc/{pid}/{name}", "rb") as handle:
            return handle.read()
    except OSError:
        return b""


def processes():
    """(pid, argv, environ) of every process this user can read; zombies have no argv and are left out."""
    for entry in os.listdir("/proc"):
        if not entry.isdigit() or int(entry) == os.getpid():
            continue
        argv = [part.decode("utf-8", "replace") for part in proc_text(entry, "cmdline").split(b"\0") if part]
        if not argv:
            continue
        env = {}
        for item in proc_text(entry, "environ").split(b"\0"):
            key, _, value = item.partition(b"=")
            if key:
                env[key.decode("utf-8", "replace")] = value.decode("utf-8", "replace")
        yield int(entry), argv, env


def family(install, exclude=(), arg=None):
    """[(pid, argv)] of the app installed at `install`, minus `exclude`, optionally only those started with `arg`."""
    found = []
    for pid, argv, env in processes():
        if pid in exclude or "--version" in argv[1:]:
            continue
        if argv[0] != install and env.get("APPIMAGE") != install:
            continue
        if arg is not None and arg not in argv[1:]:
            continue
        found.append((pid, argv))
    return sorted(found)


def socket_owner(path):
    """pids holding a listening unix socket bound to `path`."""
    inodes = set()
    try:
        with open("/proc/net/unix", encoding="utf-8", errors="replace") as handle:
            next(handle, None)
            for line in handle:
                parts = line.split(None, 7)
                if len(parts) == 8 and parts[7].strip() == path and int(parts[3], 16) & 0x10000:  # __SO_ACCEPTCON
                    inodes.add(parts[6])
    except OSError:
        return []
    owners = []
    if not inodes:
        return owners
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            for fd in os.listdir(f"/proc/{entry}/fd"):
                target = os.readlink(f"/proc/{entry}/fd/{fd}")
                if target.startswith("socket:[") and target[8:-1] in inodes:
                    owners.append(int(entry))
                    break
        except OSError:
            continue
    return sorted(owners)


# ---- checks --------------------------------------------------------------------------------------------------------

def classify(entries, version, arch):
    """Counts of manifest, signature and asset requests, plus everything else (as 'METHOD path')."""
    wanted = {
        "manifest": MANIFEST_PATH,
        "signature": MANIFEST_PATH + ".minisig",
        "asset": f"/{REPO}/releases/download/v{version}/Dockering-{arch}.AppImage",
    }
    counts = {name: 0 for name in wanted}
    other = []
    for entry in entries:
        path = str(entry.get("path", "")).split("?", 1)[0]
        name = next((n for n, p in wanted.items() if p == path), None)
        if name:
            counts[name] += 1
        else:
            other.append(f"{entry.get('method')} {entry.get('path')}")
    return counts, other


def check_requests(args):
    entries = read_log(args.log)
    counts, other = classify(entries, args.version, args.arch)
    bad_status = [f"{e.get('method')} {e.get('path')} -> {e.get('status')}" for e in entries if e.get("status") != 200]
    summary = f"manifest {counts['manifest']}, .minisig {counts['signature']}, asset {counts['asset']}"
    if all(n == 1 for n in counts.values()) and not other and not bad_status:
        return say(args.name, "PASS", f"{summary}; {len(entries)} requests, all 200")
    why = []
    if other:
        why.append("other: " + "; ".join(other[:3]))
    if bad_status:
        why.append("not 200: " + "; ".join(bad_status[:3]))
    return say(args.name, "FAIL", f"{summary}, expected one of each" + (f" ({'; '.join(why)})" if why else ""))


def check_schedule(args):
    entries = read_log(args.log)
    if not entries:
        return say(args.name, "FAIL", "no request was logged")
    first = min(parse_ts(e["ts"]) for e in entries)
    delta = first - args.start
    if delta < args.first_check - SCHEDULE_SLACK:
        return say(args.name, "FAIL", f"the first request came {delta:.1f}s after the start, before the first check was due ({args.first_check}s)")
    if delta > args.first_check + SCHEDULE_LATE:
        return say(args.name, "FAIL", f"the first request came {delta:.1f}s after the start, over {SCHEDULE_LATE:.0f}s late ({args.first_check}s)")
    return say(args.name, "PASS", f"first request {delta:.1f}s after the start (due after {args.first_check}s)")


def check_user_agent(args):
    entries = read_log(args.log)
    if not entries:
        return say(args.name, "FAIL", "no request was logged")
    seen = sorted({str(e.get("user_agent")) for e in entries})
    if seen == [args.expect]:
        return say(args.name, "PASS", f"{len(entries)} requests, all {args.expect!r}")
    return say(args.name, "FAIL", f"expected only {args.expect!r}, saw {seen}")


def check_state(args):
    try:
        with open(args.file, encoding="utf-8") as handle:
            state = json.load(handle)
    except (OSError, ValueError) as error:
        return say(args.name, "FAIL", f"state.json unreadable: {error}")
    updates = state.get("updates") or {}
    notified, last = updates.get("notified_version"), updates.get("last_run_version")
    if args.want == "toast":
        if notified == args.version:
            return say(args.name, "PASS", f"state.json records the announcement: notified_version {notified}")
        return say(args.name, "FAIL", f"state.json notified_version is {notified!r}, expected {args.version}: no notification was recorded")
    window = state.get("window")
    problems = []
    if not isinstance(window, dict) or not (window.get("width", 0) > 0 and window.get("height", 0) > 0):
        problems.append(f"the saved window bounds are gone ({window!r})")
    if last != args.version:
        problems.append(f"last_run_version is {last!r}, expected {args.version}")
    if notified != args.version:
        problems.append(f"notified_version is {notified!r}, expected {args.version}")
    if problems:
        return say(args.name, "FAIL", "; ".join(problems))
    return say(args.name, "PASS", f"window {window['width']:.0f}x{window['height']:.0f} kept, last_run_version {last}")


def check_handshake(args):
    try:
        with open(args.log, encoding="utf-8", errors="replace") as handle:
            lines = [ANSI.sub("", line.rstrip("\n")) for line in handle]
    except OSError as error:
        return say("handshake", "FAIL", f"no log: {error.strerror}")
    waits = [line for line in lines if "update relaunch: wait finished" in line]
    timeouts = [line for line in lines if "update relaunch: the previous instance did not exit in time" in line]
    if timeouts:
        return say("handshake", "FAIL", "the relaunched process timed out waiting for the old one: " + timeouts[-1][-160:])
    if len(waits) != 1:
        return say("handshake", "FAIL", f"{len(waits)} 'update relaunch: wait finished' lines in the log, expected exactly one")
    if not re.search(r"outcome=Eof\b", waits[0]):
        return say("handshake", "FAIL", "the wait did not end with end-of-file: " + waits[0][-160:])
    waited = re.search(r"waited_ms=(\d+)", waits[0])
    return say("handshake", "PASS", f"relaunch wait ended with Eof after {waited.group(1) if waited else '?'} ms")


def mount_holders(mountpoint):
    """[(pid, how, detail)]: every process of this user that pins `mountpoint`: an open file descriptor, a memory mapping, its
    working directory or its root. A FUSE mount whose holders are gone is released by the runtime; one that has a holder is busy
    and its daemon stays (this is how a relaunched app would keep the old mount of a replaced AppImage alive)."""
    prefix = mountpoint.rstrip("/") + "/"
    held = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        base = f"/proc/{entry}"
        for name in ("cwd", "root", "exe"):
            try:
                target = os.readlink(f"{base}/{name}")
            except OSError:
                continue
            if target == mountpoint or target.startswith(prefix):
                held.append((int(entry), name, target))
        try:
            for fd in os.listdir(f"{base}/fd"):
                try:
                    target = os.readlink(f"{base}/fd/{fd}")
                except OSError:
                    continue
                if target == mountpoint or target.startswith(prefix):
                    held.append((int(entry), f"fd {fd}", target))
        except OSError:
            pass
        try:
            with open(f"{base}/maps", encoding="utf-8", errors="replace") as maps:
                seen = {line.split(None, 5)[5].strip() for line in maps if len(line.split(None, 5)) == 6}
        except OSError:
            continue
        for path in sorted(seen):
            if path.startswith(prefix):
                held.append((int(entry), "map", path))
    return held


def main(argv):
    parser = argparse.ArgumentParser(prog="e2e_probe.py", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    hold = sub.add_parser("holders")
    hold.add_argument("mountpoint")

    fam = sub.add_parser("family")
    fam.add_argument("install")
    fam.add_argument("--exclude", nargs="*", type=int, default=[])
    fam.add_argument("--arg")
    fam.add_argument("-v", dest="verbose", action="store_true")

    own = sub.add_parser("owner")
    own.add_argument("socket")

    req = sub.add_parser("requests")
    req.add_argument("log")
    req.add_argument("--version", required=True)
    req.add_argument("--arch", default="x86_64")
    req.add_argument("--name", default="requests")

    sch = sub.add_parser("schedule")
    sch.add_argument("log")
    sch.add_argument("--start", type=float, required=True)
    sch.add_argument("--first-check", type=float, required=True)
    sch.add_argument("--name", default="before_requests")

    ua = sub.add_parser("user_agent")
    ua.add_argument("log")
    ua.add_argument("--expect", required=True)
    ua.add_argument("--name", default="user_agent")

    st = sub.add_parser("state")
    st.add_argument("file")
    st.add_argument("--version", required=True)
    st.add_argument("--want", choices=("toast", "kept"), required=True)
    st.add_argument("--name")

    hs = sub.add_parser("handshake")
    hs.add_argument("log")

    args = parser.parse_args(argv)
    if args.command == "holders":
        for pid, how, detail in mount_holders(args.mountpoint):
            print(f"pid {pid} {how} {detail}")
        return 0
    if args.command == "family":
        for pid, command in family(args.install, set(args.exclude), args.arg):
            print(f"{pid} {' '.join(command)}" if args.verbose else pid)
        return 0
    if args.command == "owner":
        for pid in socket_owner(args.socket):
            print(pid)
        return 0
    if args.command == "state" and not args.name:
        args.name = "toast" if args.want == "toast" else "state_kept"
    handler = {
        "requests": check_requests,
        "schedule": check_schedule,
        "user_agent": check_user_agent,
        "state": check_state,
        "handshake": check_handshake,
    }[args.command]
    return handler(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
