#!/usr/bin/env python3
"""Reads tools/engine-rootfs/pins.toml for build.sh and the workflow (REL-088).

The pin check itself is `cargo xtask engine-rootfs-pin-check` (REL-089); this file only extracts
values and renders the result of a build. Run it as `python3 -B -I pins.py ...` (no argument comes from
the network; the TOML file is part of the repository).

  pins.py get <key.path> [...]            one scalar, or one array element per line
  pins.py arch-get <image> <arch> <key>   [images.<image>.arch.<arch>] key
  pins.py apt-specs <image>               `name=version` lines of [images.<image>.apt_packages]
  pins.py entry <image> <arch> <tar.xz> <manifest> <tag> [<uncompressed-tar-sha256>]
                                          the JSON entry of one built asset
  pins.py verify <dir> <tag> [--partial]  every dockering-engine-<image>-<arch>.json in <dir> (all four
                                          unless --partial) matches its .tar.xz (name, size, sha256), its
                                          .sha256 file and the release tag in its url
  pins.py render <dir-with-entry-json> <tag>
                                          the [images.*.arch.*.asset] TOML for pins.toml
  pins.py notes <dir-with-entry-json> <tag>
                                          the release notes (markdown) of rootfs-<n>
  pins.py check-uploaded <dir> <release-json>
                                          every file in <dir> is a release asset of the same size and
                                          sha256 digest (`gh release view <tag> --json assets`)
"""

import hashlib
import json
import sys
import tomllib
from pathlib import Path

PINS = Path(__file__).resolve().with_name("pins.toml")
IMAGES = ("alpine", "ubuntu")
ARCHES = ("x86_64", "aarch64")


def load():
    with PINS.open("rb") as f:
        return tomllib.load(f)


def dig(data, path):
    node = data
    for part in path.split("."):
        if not isinstance(node, dict) or part not in node:
            raise SystemExit(f"pins.py: no key {path!r} in {PINS.name}")
        node = node[part]
    return node


def scalar(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


def emit(value):
    if isinstance(value, list):
        for item in value:
            print(scalar(item))
    elif isinstance(value, dict):
        raise SystemExit("pins.py: the key is a table, not a value")
    else:
        print(scalar(value))


def key_packages(manifest):
    """The `name version` lines under `key_packages:` in the manifest file."""
    packages = []
    on = False
    for line in Path(manifest).read_text(encoding="utf-8").splitlines():
        if line == "key_packages:":
            on = True
        elif on and line.endswith(":") and " " not in line:
            break
        elif on and line.strip():
            name, _, version = line.partition(" ")
            packages.append(f"{name}={version}")
    return packages


def entry(image, arch, tarxz, manifest, tag, tar_sha256=None):
    data = load()
    cell = dig(data, f"images.{image}.arch.{arch}")
    path = Path(tarxz)
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    name = path.name
    expected = f"dockering-engine-{image}-{arch}.tar.xz"
    if name != expected:
        raise SystemExit(f"pins.py: asset is {name}, expected {expected}")
    return {
        "image": image,
        "arch": arch,
        "name": name,
        "size": path.stat().st_size,
        "sha256": digest.hexdigest(),
        "url": f"https://github.com/{data['repo']}/releases/download/{tag}/{name}",
        "base_url": cell["base_url"],
        "base_size": cell["base_size"],
        "base_sha256": cell["base_sha256"],
        "source_date_epoch": data["build"]["source_date_epoch"],
        "packages": key_packages(manifest),
        "tar_sha256": tar_sha256,
    }


def sha256_of(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify(directory, tag, partial):
    """Fails (exit 1) unless every entry JSON in `directory` agrees with the files next to it."""
    data = load()
    problems = []
    seen = set()
    for image in IMAGES:
        for arch in ARCHES:
            stem = f"dockering-engine-{image}-{arch}"
            path = Path(directory) / f"{stem}.json"
            if not path.is_file():
                if not partial:
                    problems.append(f"missing {path.name}")
                continue
            seen.add((image, arch))
            item = json.loads(path.read_text(encoding="utf-8"))
            asset = Path(directory) / f"{stem}.tar.xz"
            want_url = f"https://github.com/{data['repo']}/releases/download/{tag}/{asset.name}"
            if not asset.is_file():
                problems.append(f"{stem}: {asset.name} is missing")
                continue
            digest = sha256_of(asset)
            sidecar = (Path(directory) / f"{stem}.tar.xz.sha256").read_text(encoding="utf-8").split()
            checks = [
                (item.get("image") == image and item.get("arch") == arch, "image/arch in the JSON"),
                (item.get("name") == asset.name, "name"),
                (item.get("size") == asset.stat().st_size, "size"),
                (item.get("sha256") == digest, "sha256"),
                (sidecar[:1] == [digest] and sidecar[1:] == [asset.name], ".sha256 file"),
                (item.get("url") == want_url, f"url (release tag {tag})"),
                (item.get("base_sha256") == data["images"][image]["arch"][arch]["base_sha256"], "base_sha256 pin"),
                (item.get("source_date_epoch") == data["build"]["source_date_epoch"], "source_date_epoch"),
            ]
            problems += [f"{stem}: {what} does not match" for ok, what in checks if not ok]
    if partial and not seen:
        problems.append("no entry found")
    if problems:
        raise SystemExit("pins.py verify: " + "; ".join(problems))
    print(f"pins.py verify: {len(seen)} asset(s) agree with their entries")


def load_entries(directory):
    entries = {}
    for path in sorted(Path(directory).glob("dockering-engine-*.json")):
        item = json.loads(path.read_text(encoding="utf-8"))
        entries[(item["image"], item["arch"])] = item
    missing = [f"{i}/{a}" for i in IMAGES for a in ARCHES if (i, a) not in entries]
    if missing:
        raise SystemExit(f"pins.py: no build entry for {', '.join(missing)}")
    return entries


def notes(directory, tag):
    data = load()
    entries = load_entries(directory)
    repo = data["repo"]
    lines = [
        f"Root filesystems for the engine that Dockering sets up in WSL (`{tag}`).",
        "",
        "These files are used by Dockering's *Set up Docker in WSL*. The app pins each file's URL, size and SHA-256, "
        "downloads it only after you agree, and imports it only if the digest matches. They are not meant to be "
        "downloaded by hand, and they are separate from the app releases.",
        "",
        "| Image | Architecture | File | Size (bytes) | SHA-256 |",
        "|---|---|---|---|---|",
    ]
    for image in IMAGES:
        for arch in ARCHES:
            e = entries[(image, arch)]
            lines.append(f"| {data['images'][image]['label']} | {arch} | `{e['name']}` | {e['size']} | `{e['sha256']}` |")
    lines += ["", "## Contents", ""]
    for image in IMAGES:
        e = entries[(image, "x86_64")]
        lines.append(f"- **{data['images'][image]['label']}**: base `{e['base_url']}` (SHA-256 `{e['base_sha256']}`); "
                     + ", ".join(f"`{p.replace('=', ' ')}`" for p in e["packages"]) + ".")
    lines += [
        "",
        "`/etc/dockering-engine-manifest.txt` inside each image lists every installed package and version; "
        "`dockering-engine-<image>-<arch>.manifest.txt` is a copy. Licence notices are in `LICENSES-<image>-<arch>.txt`.",
        "",
        "## Verify a download",
        "",
        "```sh",
        "sha256sum --check SHA256SUMS",
        f"gh attestation verify <file> -R {repo}",
        "```",
        "",
        "Built by `.github/workflows/engine-rootfs.yml` from `tools/engine-rootfs/` (see `docs/engine-rootfs.md`).",
    ]
    print(chr(10).join(lines))


def check_uploaded(directory, release_json):
    """Every file in `directory` is a release asset with the same size and sha256 digest."""
    release = json.loads(Path(release_json).read_text(encoding="utf-8"))
    published = {a["name"]: a for a in release.get("assets", [])}
    problems = []
    files = sorted(f for f in Path(directory).iterdir() if f.is_file())
    for f in files:
        asset = published.get(f.name)
        if asset is None:
            problems.append(f"{f.name}: not in the release")
            continue
        if asset.get("size") != f.stat().st_size:
            problems.append(f"{f.name}: size {asset.get('size')} != {f.stat().st_size}")
        digest = asset.get("digest")
        if digest is None:
            problems.append(f"{f.name}: GitHub reports no digest")
        elif digest != f"sha256:{sha256_of(f)}":
            problems.append(f"{f.name}: digest {digest} != sha256:{sha256_of(f)}")
    if not files:
        problems.append("no files to compare")
    if problems:
        raise SystemExit("pins.py check-uploaded: " + "; ".join(problems))
    print(f"pins.py check-uploaded: {len(files)} asset(s) match the release")


def render(directory, tag):
    entries = load_entries(directory)
    print("# Paste over the four [images.*.arch.*.asset] tables of tools/engine-rootfs/pins.toml,")
    print(f'# set [release] tag = "{tag}", and put the same url/size/sha256 into the Rust pin table.')
    for image in IMAGES:
        for arch in ARCHES:
            item = entries[(image, arch)]
            print()
            print(f"[images.{image}.arch.{arch}.asset]")
            print('status = "published"')
            print(f'name = "{item["name"]}"')
            print(f'url = "{item["url"]}"')
            print(f'size = {item["size"]}')
            print(f'sha256 = "{item["sha256"]}"')


def main(argv):
    if not argv:
        raise SystemExit(__doc__)
    command, rest = argv[0], argv[1:]
    data = load()
    if command == "get" and rest:
        for path in rest:
            emit(dig(data, path))
    elif command == "arch-get" and len(rest) == 3:
        image, arch, key = rest
        emit(dig(data, f"images.{image}.arch.{arch}.{key}"))
    elif command == "apt-specs" and len(rest) == 1:
        for name, version in dig(data, f"images.{rest[0]}.apt_packages").items():
            print(f"{name}={version}")
    elif command == "entry" and len(rest) in (5, 6):
        print(json.dumps(entry(*rest), indent=2, sort_keys=True))
    elif command == "verify" and len(rest) in (2, 3) and (len(rest) == 2 or rest[2] == "--partial"):
        verify(rest[0], rest[1], len(rest) == 3)
    elif command == "render" and len(rest) == 2:
        render(*rest)
    elif command == "notes" and len(rest) == 2:
        notes(*rest)
    elif command == "check-uploaded" and len(rest) == 2:
        check_uploaded(*rest)
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
