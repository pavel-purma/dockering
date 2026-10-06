# SignPath artifact configurations (REL-031)

The release workflow signs Windows files through [SignPath](https://signpath.io) when the
repository variable `WINDOWS_SIGNING` is `signpath` (setup: [docs/signing.md](../../docs/signing.md)).

| File | SignPath slug | Signs |
|---|---|---|
| `exe.xml` | `exe` | `dockering.exe`, uploaded as `unsigned-exe-<target>` before packaging |
| `installer.xml` | `installer` | `Dockering-Setup-<x64\|arm64>.exe`, uploaded as `unsigned-setup-<target>` |

Import each file into the SignPath project as an artifact configuration with that slug. SignPath
doesn't read them from the repository, so a change here must be repeated there.

- Both take the parameter `version`; `release.yml` passes the workspace version (`parameters:` input of
  the signing step).
- `product-name` and `product-version` pin the product metadata that SignPath Foundation's terms ask
  for: the product name is the project's name and the version is the same in every file of a build.
  `build.rs` (REL-027) and `dockering.iss` write the full SemVer string, including a pre-release
  suffix, into the `ProductVersion` of the exe and the installer.
- Inno's generated uninstaller is not signed by this flow (known gap, REL-030).
