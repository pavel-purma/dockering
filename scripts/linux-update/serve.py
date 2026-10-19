#!/usr/bin/env python3
"""Static release server of the Linux update end-to-end test (REL-085; plan linux-appimage-updates §6.5).

    python3 serve.py --root DIR --port N [--log FILE] [--control FILE]

Serves the files below DIR (`cargo xtask update-e2e-fixture` writes `DIR/root`, a tree that mirrors GitHub's paths) over
plain HTTP, on 127.0.0.1 only: the app under test runs in the same container, and the seam
(`DOCKERING_UPDATE_E2E_URL=http://127.0.0.1:N`) replaces only scheme and host of what it fetches, so the path is kept.
GET and HEAD only; no directory listings; a `..` segment is refused with 403.

--port 0 picks a free port. The first line on stdout, printed once the socket is listening, is
`serve.py: listening on 127.0.0.1:<port>`.

--log FILE appends one JSON object per request, written before the response goes out, so a client that has its
answer can read the line: {"ts": "<UTC, ISO 8601, ms>", "method": ..., "path": <request target as sent>,
"user_agent": <User-Agent header, or null>, "status": <HTTP status>}. `ts` is when the request arrived.

--control FILE is the fault switch, read again on EVERY request, so a scenario toggles it while the server runs
(`echo down > FILE`; write it atomically with `mv` if a torn read matters):
    ok     (also: no file, an empty file)  serve normally
    down   every request gets 503
    slow   every request is answered after 2 s
Any other content makes every request fail with 500, so a typo cannot pass as `ok`.

SIGTERM and SIGINT stop it cleanly (exit status 0). Standard library only.
"""

import argparse
import json
import os
import signal
import socketserver
import sys
import threading
import time
import urllib.parse
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIND = "127.0.0.1"
SLOW_SECONDS = 2.0
MODES = ("ok", "down", "slow")
CHUNK = 1 << 16
TYPES = {
    ".json": "application/json",
    ".minisig": "text/plain; charset=utf-8",
    ".txt": "text/plain; charset=utf-8",
}


def utc_now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"


def status_of(code):
    """The status as a plain int (the standard library passes an HTTPStatus or '-')."""
    try:
        return int(code)
    except (TypeError, ValueError):
        return None


def resolve(root, target):
    """The file a request target names below `root`: (path, None), or (None, HTTP status).

    The target is split on `/` only after percent-decoding, so `%2e%2e` and `..%2f` are `..` too.
    """
    path = urllib.parse.unquote(urllib.parse.urlsplit(target).path)
    if "\0" in path or "\\" in path:
        return None, 400
    parts = [part for part in path.split("/") if part not in ("", ".")]
    if ".." in parts:
        return None, 403
    full = os.path.join(root, *parts)
    if not os.path.isfile(full):
        return None, 404
    return full, None


class ReleaseServer(ThreadingHTTPServer):
    daemon_threads = True  # a client holding a connection never blocks shutdown

    def __init__(self, port, root, log_path, control_path):
        # before the socket exists: a failed bind still runs server_close()
        self.root = root
        self.control_path = control_path
        self.lock = threading.Lock()
        self.log = None
        super().__init__((BIND, port), Handler)
        self.log = open(log_path, "a", encoding="utf-8") if log_path else None

    def server_bind(self):
        # not HTTPServer.server_bind: its getfqdn() asks the resolver, which a container may not have
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]

    def fault_mode(self):
        """The mode in the control file now. Anything that is not one of MODES is reported as such."""
        if not self.control_path:
            return "ok"
        try:
            with open(self.control_path, encoding="utf-8") as handle:
                text = handle.read(64)
        except FileNotFoundError:
            return "ok"
        except OSError as error:
            return f"unreadable control file ({error.strerror})"
        return text.strip() or "ok"

    def record(self, entry):
        line = json.dumps(entry, separators=(",", ":"))
        with self.lock:
            if self.log:
                self.log.write(line + "\n")
                self.log.flush()
            print(
                f"serve.py: {entry['method']} {entry['path']} -> {entry['status']}",
                file=sys.stderr,
                flush=True,
            )

    def server_close(self):
        super().server_close()
        if self.log:
            self.log.close()

    def handle_error(self, request, client_address):
        # a client that hangs up mid-download is not an error of the server
        if not isinstance(sys.exc_info()[1], (BrokenPipeError, ConnectionResetError)):
            super().handle_error(request, client_address)


class Handler(BaseHTTPRequestHandler):
    server_version = "dockering-e2e-serve"
    sys_version = ""

    def parse_request(self):
        self.received = utc_now()
        return super().parse_request()

    def log_request(self, code="-", size="-"):
        # send_response() calls this before the headers go out; send_error() reaches it too
        headers = getattr(self, "headers", None)
        self.server.record(
            {
                "ts": getattr(self, "received", utc_now()),
                "method": getattr(self, "command", None),
                "path": getattr(self, "path", None),
                "user_agent": headers.get("User-Agent") if headers is not None else None,
                "status": status_of(code),
            }
        )

    def log_message(self, format, *args):  # noqa: A002  (the name is the base class's)
        print("serve.py: " + format % args, file=sys.stderr, flush=True)

    def text(self, status, message, head=False, extra=()):
        body = (message + "\n").encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        for name, value in extra:
            self.send_header(name, value)
        self.end_headers()
        if not head:
            self.wfile.write(body)

    def serve(self, head):
        mode = self.server.fault_mode()
        if mode == "slow":
            time.sleep(SLOW_SECONDS)
        elif mode == "down":
            return self.text(503, "service unavailable (fault switch: down)", head)
        elif mode != "ok":
            return self.text(500, f"unknown fault mode {mode!r}; use one of {', '.join(MODES)}", head)
        full, status = resolve(self.server.root, self.path)
        if status is not None:
            reasons = {400: "bad path", 403: "path traversal refused", 404: "not found"}
            return self.text(status, reasons[status], head)
        try:
            with open(full, "rb") as handle:
                size = os.fstat(handle.fileno()).st_size
                self.send_response(200)
                self.send_header("Content-Type", TYPES.get(os.path.splitext(full)[1], "application/octet-stream"))
                self.send_header("Content-Length", str(size))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                while not head:
                    chunk = handle.read(CHUNK)
                    if not chunk:
                        break
                    self.wfile.write(chunk)
        except OSError as error:
            if not isinstance(error, (BrokenPipeError, ConnectionResetError)):
                print(f"serve.py: {full}: {error}", file=sys.stderr, flush=True)

    def do_GET(self):
        self.serve(head=False)

    def do_HEAD(self):
        self.serve(head=True)

    def refuse_method(self):
        self.text(405, "only GET and HEAD", extra=[("Allow", "GET, HEAD")])

    do_POST = do_PUT = do_DELETE = do_PATCH = do_OPTIONS = refuse_method


def port_number(text):
    try:
        port = int(text)
    except ValueError:
        port = -1
    if not 0 <= port <= 65535:
        raise argparse.ArgumentTypeError(f"{text!r} is not a port (0-65535)")
    return port


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="serve.py",
        description="Static HTTP server of the Linux update end-to-end test; binds 127.0.0.1 only.",
        epilog="Fault switch (--control): ok (default), down (503), slow (2 s delay); read on every request.",
    )
    parser.add_argument("--root", required=True, help="directory to serve")
    parser.add_argument("--port", required=True, type=port_number, help="TCP port on 127.0.0.1 (0 picks a free one)")
    parser.add_argument("--log", metavar="FILE", help="append one JSON line per request to FILE (requests.jsonl)")
    parser.add_argument("--control", metavar="FILE", help="fault switch file: ok, down or slow")
    args = parser.parse_args(argv)
    if not os.path.isdir(args.root):
        parser.error(f"--root {args.root!r} is not a directory")
    try:
        server = ReleaseServer(args.port, os.path.abspath(args.root), args.log, args.control)
    except OSError as error:
        parser.exit(1, f"serve.py: cannot listen on {BIND}:{args.port}: {error.strerror}\n")

    def stop(signum, frame):
        # shutdown() waits for serve_forever(), which runs on this thread: ask from another one
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    print(f"serve.py: listening on {BIND}:{server.server_address[1]}", flush=True)
    try:
        server.serve_forever(poll_interval=0.1)
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
