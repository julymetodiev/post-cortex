#!/usr/bin/env python3
"""Every dependency at its latest release (CLAUDE.md, LAW: every dependency
at its latest version; owner 2026-11-17).

    scripts/check-deps.sh [REPO]

REPO is this repository by default (../post-cortex-cloud for the cloud). Every
lock file git tracks there is checked:

- Cargo.lock: `cargo update --dry-run` takes nothing (no newer compatible
  release), and `cargo upgrade --dry-run --incompatible` (cargo-edit) finds no
  direct dependency with a newer major release;
- package-lock.json: `npm outdated --all` finds no package below the release
  its range wants, and no direct one below its latest release;
- uv.lock: `uv lock --upgrade --dry-run` updates nothing;
- pubspec.lock: `flutter pub outdated` finds no package below its upgradable
  release, and no direct or dev one below its latest release;
- the workflows under .github/workflows: each `uses: owner/repo@vN` is the
  newest major of its releases.

A dependency kept below its latest release is a line of
scripts/deps-exceptions.txt in REPO:

    <ecosystem> <name> <version kept> <why>

with ecosystem cargo, npm, uv, pub or action. A listed name is not reported
while its newest release is the one the line names as not taken (the why says
which); an exception that no longer holds back anything is itself an error, so
the file only ever lists what is real.
"""

import json
import os
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

ECOSYSTEMS = ("cargo", "npm", "uv", "pub", "action")


def run(cmd, cwd):
    """The command's stdout and stderr together; a command that is missing
    is an error of the check, not a pass."""
    try:
        done = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        sys.exit(f"check-deps: {cmd[0]} is not installed")
    return done.stdout + done.stderr


def version_key(text):
    """A release as a sortable key: numbers first, a pre-release below its
    release."""
    text = text.lstrip("v")
    core, _, pre = text.partition("-")
    core = core.split("+")[0]
    nums = [int(n) if n.isdigit() else 0 for n in core.split(".")]
    while len(nums) < 3:
        nums.append(0)
    return (nums, 0 if pre else 1, pre)


def newer(latest, current):
    return version_key(latest) > version_key(current)


def tracked(repo, name):
    out = run(["git", "ls-files", f"*{name}", name], repo)
    return sorted({repo / line for line in out.splitlines() if line.strip()})


def cargo(lock):
    """(lock updates, direct majors) of the Cargo project of `lock`."""
    cwd = lock.parent
    updates = []
    for line in run(["cargo", "update", "--dry-run"], cwd).splitlines():
        m = re.match(r"\s*(Updating|Adding|Removing|Downgrading) (\S+) (v\S+)(?: -> (v\S+))?", line)
        if m and m.group(2) != "crates.io":
            updates.append(f"{m.group(2)} {m.group(3)}" + (f" -> {m.group(4)}" if m.group(4) else f" ({m.group(1).lower()})"))
    majors = {}
    for line in run(["cargo", "upgrade", "--dry-run", "--incompatible"], cwd).splitlines():
        cols = line.split()
        if len(cols) < 5 or cols[0] in ("name", "====") or not re.match(r"^\d", cols[3]):
            continue
        name, req, compatible, latest = cols[0], cols[1], cols[2], cols[3]
        if compatible != latest and newer(latest, compatible if re.match(r"^\d", compatible) else req):
            majors[name] = (req, latest)
    return updates, majors


def newest_npm(name, cwd):
    """The highest version of `name` published on the registry."""
    out = run(["npm", "view", name, "versions", "--json"], cwd)
    start = out.find("[")
    versions = json.loads(out[start:out.rfind("]") + 1]) if start >= 0 else []
    return max(versions, key=version_key) if versions else None


def npm(lock):
    """(lock updates, direct packages outside their range) of the npm project
    of `lock`. The lock is behind when `npm update` would change it; a
    package pinned exactly by one dependent keeps one copy for all, so the
    per-dependent `wanted` of `npm outdated --all` is no measure of it."""
    cwd = lock.parent
    if not (cwd / "node_modules").is_dir():
        # npm reads what is installed: without it nothing is current.
        run(["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"], cwd)
    out = run(["npm", "update", "--dry-run", "--json", "--ignore-scripts", "--no-audit", "--no-fund"], cwd)
    start = out.find("{")
    plan = json.loads(out[start:out.rfind("}") + 1]) if start >= 0 else {}
    changes = sum(plan.get(k, 0) for k in ("added", "changed", "removed"))
    updates = [f"npm update would change {changes} packages"] if changes else []
    out = run(["npm", "outdated", "--json", "--workspaces", "--include-workspace-root"]
              if json.loads((cwd / "package.json").read_text()).get("workspaces")
              else ["npm", "outdated", "--json"], cwd)
    start = out.find("{")
    data = json.loads(out[start:]) if start >= 0 else {}
    majors = {}
    for name, entries in data.items():
        for e in entries if isinstance(entries, list) else [entries]:
            current, latest = e.get("current"), e.get("latest")
            if not current:
                continue
            if "-" in current:
                # A pre-release line (DeepSeek Harness publishes under `next`):
                # the `latest` tag can be older than it, so the newest release
                # published is the one to compare with.
                latest = newest_npm(name, cwd) or latest
            if latest and newer(latest, current):
                majors[name] = (current, latest)
    return updates, majors


def uv(lock):
    updates = [line.strip() for line in run(["uv", "lock", "--upgrade", "--dry-run"], lock.parent).splitlines()
               if line.startswith(("Update ", "Add ", "Remove "))]
    return updates, {}


def pub(lock):
    out = run(["flutter", "pub", "outdated", "--json"], lock.parent)
    start = out.find("{")
    if start < 0:
        sys.exit(f"check-deps: flutter pub outdated in {lock.parent} gave no answer:\n{out[-800:]}")
    updates, majors = [], {}
    for p in json.loads(out[start:])["packages"]:
        current = (p.get("current") or {}).get("version")
        upgradable = (p.get("upgradable") or {}).get("version")
        latest = (p.get("latest") or {}).get("version")
        if not current:
            continue
        resolvable = (p.get("resolvable") or {}).get("version")
        if upgradable and newer(upgradable, current):
            updates.append(f"{p['package']} {current} -> {upgradable}")
        elif p.get("kind") in ("direct", "dev") and resolvable and newer(resolvable, current):
            # Our constraint holds it back. One the Flutter SDK pins
            # (resolvable == current) moves with the SDK, checked below.
            majors[p["package"]] = (current, latest or resolvable)
    return updates, majors


def flutter_sdk():
    """The Flutter SDK against the newest stable release, as (have, latest)
    when it is behind."""
    out = run(["flutter", "--version", "--machine"], ".")
    have = json.loads(out[out.find("{"):out.rfind("}") + 1])["frameworkVersion"]
    url = "https://storage.googleapis.com/flutter_infra_release/releases/releases_macos.json"
    with urllib.request.urlopen(url, timeout=20) as resp:
        releases = json.load(resp)
    stable = releases["current_release"]["stable"]
    latest = next(r["version"] for r in releases["releases"] if r["hash"] == stable)
    return (have, latest) if newer(latest, have) else None


def actions(repo):
    """Each action a workflow uses at a major below its newest release."""
    uses = set()
    for wf in sorted((repo / ".github" / "workflows").glob("*.y*ml")):
        for m in re.finditer(r"uses:\s*([\w.-]+/[\w.-]+)(?:/[\w./-]+)?@(v?\d[\w.]*)", wf.read_text()):
            uses.add((m.group(1), m.group(2)))
    majors = {}
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    for name, ref in sorted(uses):
        req = urllib.request.Request(f"https://api.github.com/repos/{name}/releases/latest",
                                     headers={"Accept": "application/vnd.github+json", "User-Agent": "check-deps"})
        if token:
            req.add_header("Authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                tag = json.load(resp)["tag_name"]
        except OSError as err:
            sys.exit(f"check-deps: the releases of {name}: {err}")
        if version_key(tag)[0][0] > version_key(ref)[0][0]:
            majors[name] = (ref, tag)
    return majors


def exceptions(repo):
    path = repo / "scripts" / "deps-exceptions.txt"
    kept = {}
    if not path.exists():
        return kept, path
    for n, line in enumerate(path.read_text().splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        cols = line.split(None, 3)
        if len(cols) < 4 or cols[0] not in ECOSYSTEMS:
            sys.exit(f"check-deps: {path}:{n}: '<ecosystem> <name> <version kept> <why>', ecosystem one of {', '.join(ECOSYSTEMS)}")
        kept[(cols[0], cols[1])] = cols[2]
    return kept, path


def main():
    repo = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent).resolve()
    kept, kept_path = exceptions(repo)
    used = set()
    problems = []
    checks = [("cargo", "Cargo.lock", cargo), ("npm", "package-lock.json", npm),
              ("uv", "uv.lock", uv), ("pub", "pubspec.lock", pub)]
    for eco, name, check in checks:
        for lock in tracked(repo, name):
            rel = lock.relative_to(repo)
            updates, majors = check(lock)
            for u in updates:
                problems.append(f"{rel}: {u} (a compatible release: update the lock file)")
            for dep, (have, latest) in sorted(majors.items()):
                if (eco, dep) in kept:
                    used.add((eco, dep))
                    continue
                problems.append(f"{rel}: {dep} {have}, latest {latest} (outside its version range: raise it, with the code changes it needs)")
    if tracked(repo, "pubspec.lock"):
        behind = flutter_sdk()
        if behind:
            problems.append(f"flutter: the SDK is {behind[0]}, latest stable {behind[1]} (flutter upgrade)")
    for dep, (have, latest) in sorted(actions(repo).items()):
        if ("action", dep) in kept:
            used.add(("action", dep))
            continue
        problems.append(f".github/workflows: {dep}@{have}, latest {latest}")
    for key, version in sorted(kept.items()):
        if key not in used:
            problems.append(f"{kept_path.relative_to(repo)}: {key[0]} {key[1]} {version} holds back nothing any more: remove the line")
    for p in problems:
        print(p)
    if problems:
        print(f"check-deps: {len(problems)} behind their latest release in {repo.name}")
        return 1
    print(f"check-deps: every dependency of {repo.name} at its latest release")
    return 0


if __name__ == "__main__":
    sys.exit(main())
