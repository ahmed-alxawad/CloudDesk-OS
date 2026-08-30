#!/usr/bin/env python3
"""Generate a CycloneDX 1.5 SBOM for CloudDesk-OS's shipped native binaries
and frontend production dependencies, from `cargo metadata`/`cargo tree`
and `npm list` -- no fabricated versions, no external SBOM tool.

Usage:
    packaging/gen-sbom.py <version> > sbom.cdx.json

Must be run from the repository root (or REPO_ROOT env var set).
"""
import json
import os
import subprocess
import sys

REPO_ROOT = os.environ.get("REPO_ROOT", os.getcwd())


def reachable(binary_pkg):
    out = subprocess.run(
        ["cargo", "tree", "-p", binary_pkg, "-e", "normal", "--prefix", "none"],
        cwd=REPO_ROOT,
        capture_output=True, text=True, check=True,
    ).stdout
    names = set()
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        line = line.rstrip(" (*)").strip()
        parts = line.split()
        if len(parts) >= 2:
            name, version = parts[0], parts[1].lstrip("v")
            names.add((name, version))
    return names


def walk_npm(node, npm_components):
    for name, dep in (node.get("dependencies") or {}).items():
        version = dep.get("version")
        if version:
            npm_components.add((name, version))
        walk_npm(dep, npm_components)


def cdx_component(name, version, ecosystem):
    purl_type = {"cargo": "cargo", "npm": "npm"}[ecosystem]
    return {
        "type": "library",
        "name": name,
        "version": version,
        "purl": f"pkg:{purl_type}/{name}@{version}",
    }


def main():
    if len(sys.argv) != 2:
        print("usage: gen-sbom.py <version>", file=sys.stderr)
        return 2
    version = sys.argv[1]

    rust_components = set()
    for bin_pkg in ("clouddeskd", "cloudesk-privd", "cloudesk-sessiond"):
        rust_components |= reachable(bin_pkg)

    npm_tree = json.loads(subprocess.run(
        ["npm", "list", "--omit=dev", "--all", "--json"],
        cwd=os.path.join(REPO_ROOT, "apps/web"),
        capture_output=True, text=True, check=True,
    ).stdout)
    npm_components = set()
    walk_npm(npm_tree, npm_components)

    components = [cdx_component(n, v, "cargo") for n, v in sorted(rust_components)]
    components += [cdx_component(n, v, "npm") for n, v in sorted(npm_components)]

    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "CloudDesk-OS",
                "version": version,
            },
            "tools": [{"vendor": "CloudDesk-OS engineering", "name": "gen-sbom.py (cargo metadata + npm list based)"}],
        },
        "components": components,
    }

    print(json.dumps(sbom, indent=2, sort_keys=False))
    print(f"Rust production components: {len(rust_components)}", file=sys.stderr)
    print(f"npm production components: {len(npm_components)}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
