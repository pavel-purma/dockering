#!/usr/bin/env bash
# Test double for the Dockering AppImage of the runner's self-test (selftest.sh). It is not part of the product and the real
# runs never use it. selftest.sh copies this file twice, rewriting the two assignments below: once as the OLD app (role old,
# version 0.0.1; installed as ~/.local/bin/dockering) and once as the NEW one (role new, the fixture's version; the file
# the old app swaps in). The behaviour is stub_app.py; the defect to inject comes in E2E_STUB_FLAGS (see selftest.sh).
# Like the AppImage runtime it sets APPIMAGE to its own path, which is how s1.sh tells the app's processes apart.
ROLE=old
VERSION=0.0.1
export APPIMAGE="${E2E_STUB_APPIMAGE:-$0}"
exec python3 -B "${E2E_STUB_BODY:?set E2E_STUB_BODY}" "$ROLE" "$VERSION" "$@"
