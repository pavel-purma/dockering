# Dependency patches

## gpui-pre-windows 0.3.7

Source: the [crates.io release](https://crates.io/crates/gpui-pre-windows/0.3.7),
which packages Zed revision `1a28cff4b409169bac058bca40dfbfeb7621d19b`.
The original package checksum from Cargo.lock is
`f05592a6f9e3e6bb7a9f2a0d4779271cf747a948db1c07b02448de777ffff8f3`.
The source, build script, normalized and original manifests, and Apache-2.0
license are preserved. Cargo cache markers and the upstream lockfile are omitted.

The workspace uses `[patch.crates-io]` to apply this version on Windows without
editing a developer's Cargo cache. The crate is excluded from workspace members.

Local changes in `src/directx_devices.rs` (NFR-050):

- An unsuccessful optional `DXGIGetDebugInterface1` probe logs one warning with
  its error and continues without the debug layer.
- The duplicate generic warning in `get_dxgi_factory` is removed.
- Factory/device creation and their error propagation are unchanged. Release
  builds still skip the debug-layer probe.

Local changes for a clean window close (NFR-050). Closing the last window from
the system (title-bar close, Alt+F4) used to log four spurious errors:

- `src/window.rs` (`Drop for WindowsWindow`): the deferred `active_status_change`
  callback is cleared with `visibility_change`, so a `WM_ACTIVATE` sent during
  close no longer reports against a removed GPUI window (`window not found`).
  `DestroyWindow` is skipped when `DefWindowProc` has already destroyed the HWND
  (`Invalid window handle`).
- `src/events.rs` (`handle_destroy_msg`): `RevokeDragDrop` runs in `WM_DESTROY`
  while the handle is still valid, not after it (`0x80040102`).
- `src/dispatcher.rs` (`dispatch_on_main_thread`): a wake-up posted after the
  platform window is destroyed at shutdown is ignored instead of logged.

Remove this override when upgrading to an upstream release with the same fixes.
