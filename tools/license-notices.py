#!/usr/bin/env python3
"""Writes the licence notices of everything Outpost is built with.

    python3 tools/license-notices.py > THIRD-PARTY-NOTICES.txt

Most of the libraries' licences (MIT, BSD, Apache-2.0, MPL-2.0, ...) ask that
their notice travels with programs built from them. This gathers, for every
Rust package in the build (Windows and Linux together) and every npm package
that ends up in the interface, its name, version, declared licence and the
licence files it ships. Texts that are byte-for-byte the same are printed
once and referred to.

Needs only what a build already needs: `cargo` (the packages must have been
fetched, which any build does) and `node_modules`. Run from the repository
root. Exits non-zero if a package declares no licence at all.
"""

import hashlib
import json
import os
import subprocess
import sys

TARGETS = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]
# The npm packages whose code or files are shipped inside the app.
NPM_SHIPPED = ["solid-js", "@fontsource-variable/inter", "@tauri-apps/api"]
NOTICE_NAMES = ("license", "licence", "copying", "notice", "unlicense", "copyright", "ofl")


def notice_files(directory):
    found = []
    try:
        names = sorted(os.listdir(directory))
    except OSError:
        return found
    for name in names:
        path = os.path.join(directory, name)
        if os.path.isfile(path) and name.lower().startswith(NOTICE_NAMES) and os.path.getsize(path) < 200_000:
            with open(path, "rb") as f:
                found.append((name, f.read().decode("utf-8", "replace").replace("\r\n", "\n").strip()))
    return found


def rust_packages():
    packages = {}
    for target in TARGETS:
        raw = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
            check=True, capture_output=True, text=True,
        ).stdout
        meta = json.loads(raw)
        by_id = {p["id"]: p for p in meta["packages"]}
        # Only what is linked into the app: not build scripts' or tests' own needs.
        nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
        roots = [p["id"] for p in meta["packages"] if p["name"] == "outpost-shell-tauri"]
        seen, stack = set(), list(roots)
        while stack:
            pid = stack.pop()
            if pid in seen:
                continue
            seen.add(pid)
            for dep in nodes[pid]["deps"]:
                if any(kind["kind"] in (None, "normal") for kind in dep["dep_kinds"]):
                    stack.append(dep["pkg"])
        for pid in seen:
            p = by_id[pid]
            if p["name"].startswith("outpost-"):
                continue
            packages[(p["name"], p["version"])] = (p.get("license") or "", os.path.dirname(p["manifest_path"]), p.get("repository") or "")
    return packages


def npm_packages():
    packages = {}
    for name in NPM_SHIPPED:
        directory = os.path.join("node_modules", name)
        with open(os.path.join(directory, "package.json"), encoding="utf-8") as f:
            info = json.load(f)
        repo = info.get("repository")
        repo = repo.get("url", "") if isinstance(repo, dict) else (repo or "")
        packages[(name, info["version"])] = (info.get("license") or "", directory, repo)
    return packages


def main():
    out = sys.stdout
    out.reconfigure(encoding="utf-8", newline="\n")
    texts, order, missing, bare = {}, [], [], []
    sections = [("Rust libraries", rust_packages()), ("Interface libraries and fonts", npm_packages())]
    out.write("Outpost Mail: licences of the software it is built with\n")
    out.write("=" * 56 + "\n\n")
    out.write("Outpost's own code is released under the Unlicense. It is built with the\n")
    out.write("libraries below, each of which keeps its own licence. The full texts follow\n")
    out.write("the lists; a text shared by several libraries is printed once.\n\n")
    for title, packages in sections:
        out.write(f"{title}\n{'-' * len(title)}\n\n")
        for (name, version), (licence, directory, repo) in sorted(packages.items(), key=lambda kv: kv[0][0].lower()):
            if not licence:
                missing.append(f"{name} {version}")
            refs = []
            files = notice_files(directory)
            if not files:
                bare.append(f"{name} {version}")
            for file_name, text in files:
                key = hashlib.sha256(text.encode()).hexdigest()[:12]
                if key not in texts:
                    texts[key] = text
                    order.append(key)
                refs.append(f"{file_name} [{key}]")
            out.write(f"{name} {version}\n  licence: {licence or 'NOT DECLARED'}\n")
            if repo:
                out.write(f"  source:  {repo}\n")
            out.write(f"  texts:   {', '.join(refs) if refs else '(none in the package; see the licence named above)'}\n\n")
    out.write("\nLicence texts\n-------------\n")
    for key in order:
        out.write(f"\n{'=' * 30} [{key}] {'=' * 30}\n\n{texts[key]}\n")
    print(f"{sum(len(p) for _, p in sections)} packages, {len(order)} distinct texts, "
          f"{len(bare)} packages ship no licence file", file=sys.stderr)
    if missing:
        print("No licence declared by: " + ", ".join(missing), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
