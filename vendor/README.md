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

Remove this override when upgrading to an upstream release with the same fix.
