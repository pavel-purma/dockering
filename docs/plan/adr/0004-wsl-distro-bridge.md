# ADR-0004: WSL distro Docker via named-pipe ⇄ `wsl.exe` stdio bridge

- **Status:** accepted (validate in spike S-4) · 2026-10-01

## Context

Users run Docker Engine inside WSL2 distros (for example, Ubuntu). Its Unix socket isn't reachable
from Windows. Exposing dockerd on TCP requires the user to edit `daemon.json`, and it's
unauthenticated.

## Decision

The default mode is a local named-pipe server owned by Dockering. For each client connection it
spawns `wsl.exe -d <distro> --exec docker system dial-stdio`, falling back to
`socat - UNIX-CONNECT:/var/run/docker.sock`, and pumps bytes both ways. bollard connects to the
pipe as if it were Docker Desktop's pipe. TCP mode is offered as an opt-in alternative.

## Consequences

- Zero in-distro configuration when the `docker` CLI or `socat` is present, which is almost always.
- Full Docker API fidelity, including hijacked exec and streaming.
- Each HTTP connection costs one `wsl.exe` process. hyper's connection pooling keeps the count small.
- The pipe must be ACL-restricted to the current user (NFR-021).
