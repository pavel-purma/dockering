# Vendored WSLC IDL

These are verbatim copies of the **internal** WSLC COM interface definitions from
[microsoft/WSL](https://github.com/microsoft/WSL). They are the source of truth for the hand-declared
vtables in `src/com/abi/` (ADR-0003, spec 20 §5.3).

| Directory | WSL tag | Commit | Files (upstream path) |
|---|---|---|---|
| `3.0.1/` | [`3.0.1`](https://github.com/microsoft/WSL/tree/3.0.1) | `91f161fa240dc355c1a88daabc8aac4273e35ba5` | `src/windows/service/inc/wslc.idl`, `src/windows/service/inc/WSLCShared.idl` |

`3.0.1/` was byte-compared with
`https://raw.githubusercontent.com/microsoft/WSL/3.0.1/src/windows/service/inc/{wslc.idl,WSLCShared.idl}`
on 2026-10-02 (identical, LF line endings).

`wslc.idl` imports only `unknwn.idl`, `wtypes.idl` (Windows SDK) and `WSLCShared.idl`, so no other
upstream file is needed for type information. The `WSLC_E_*` HRESULTs are defined at the end of
`wslc.idl` (`MAKE_HRESULT(SEVERITY_ERROR, FACILITY_ITF, 0x0600 + n)`).

## Rules

- Never edit these files. Add a new `<tag>/` directory for a new WSL release.
- `wslc.idl` says: "ABI breaking changes in this file are OK, since both client & server always ship
  together". Every directory therefore needs its own verified ABI module in `src/com/abi/` before
  COM is used on that WSL version; otherwise Dockering uses the `wslc.exe` CLI.
- `cargo xtask wslc-abi-check <tag|latest>` compares a WSL tag with the newest directory here
  (spec 20 §5.7).

## License

The IDL files are © Microsoft Corporation and distributed under the MIT License. The full upstream
license text is in [`LICENSE-WSL`](LICENSE-WSL) (copied from the same tag). The copyright headers in
the `.idl` files are kept unchanged.
