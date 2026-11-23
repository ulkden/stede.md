#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Enforce the 14-day rule on everything a change pins.

It compares a base revision with the working tree and checks each version the
change introduces:

  * crates.io packages in Cargo.lock (direct and transitive)
  * the Rust toolchain in rust-toolchain.toml
  * GitHub Actions in .github/workflows/ and .github/actions/
  * the dev tools pinned in the justfile

A version passes if it was published at least --min-days ago. Crates, the
toolchain and dev tools also pass on a patch bump (same major.minor) of a
version the base already had: bug fixes may come in at once. GitHub Actions get
no patch exemption, matching their flat Dependabot cooldown (see "Dependencies"
in CONTRIBUTING.md).

It also enforces exact pins: the toolchain must be a full version (not
"stable"), and every remote action must be pinned to a commit SHA with a
version comment that resolves to that SHA.

For a security fix that is not a patch bump, add the `dependency-age-exempt`
label to the PR (CI passes --exempt) and say why in the PR. The label only
waives "too new"; pinning problems and unverifiable versions still fail.

Only the Python standard library is used. Network errors fail the check: an
age that can't be confirmed doesn't pass.
"""

from __future__ import annotations

import sys

if sys.version_info < (3, 11):
    sys.exit("error: scripts/dependency-age.py needs Python 3.11 or newer (for tomllib)")

import argparse
import datetime as dt
import functools
import json
import os
import re
import subprocess
import time
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

USER_AGENT = "dependency-age-check"
CRATES_IO_SOURCES = (
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
)
# justfile variable prefix -> crates.io crate
JUSTFILE_TOOLS = {
    "just": "just",
    "deny": "cargo-deny",
    "insta": "cargo-insta",
    "typos": "typos-cli",
}
ACTION_FILES = (".github/workflows", ".github/actions")
USES_RE = re.compile(
    r"^[ \t]*-?[ \t]*uses:[ \t]*([^\s#]+)(?:[ \t]*#[ \t]*(\S+))?", re.MULTILINE
)
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
VERSION_RE = re.compile(r"^v?(\d+)\.(\d+)(?:\.(\d+))?(.*)$")


@dataclass
class Change:
    kind: str  # crate | toolchain | action | tool
    name: str
    old: str | None
    new: str
    note: str = ""


@dataclass
class Result:
    change: Change
    status: str  # ok | patch | too-new | error
    detail: str


# --- reading the base and the working tree -----------------------------------


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True
    ).stdout


def at_base(base: str, path: str) -> str:
    """File contents at the base revision, or "" if it didn't exist there."""
    try:
        return git("show", f"{base}:{path}")
    except subprocess.CalledProcessError:
        return ""


def at_head(path: str) -> str:
    p = Path(path)
    return p.read_text() if p.exists() else ""


def files_under(base: str | None, roots: tuple[str, ...]) -> list[str]:
    if base is None:
        found = []
        for root in roots:
            found += [str(p) for p in Path(root).rglob("*.y*ml")]
        return sorted(found)
    out = git("ls-tree", "-r", "--name-only", base, "--", *roots)
    return sorted(p for p in out.splitlines() if p.endswith((".yml", ".yaml")))


# --- version helpers ---------------------------------------------------------


def parse_version(v: str) -> tuple[int, int, int, str] | None:
    m = VERSION_RE.match(v)
    if not m:
        return None
    return int(m[1]), int(m[2]), int(m[3] or 0), m[4]


def is_patch_bump(old: str, new: str) -> bool:
    """Same major.minor, higher patch, and neither is a pre-release.

    Under Cargo's semver every 0.0.x release is breaking, so it never counts.
    """
    a, b = parse_version(old), parse_version(new)
    if not a or not b or a[:2] == (0, 0):
        return False
    # Allow build metadata (+...) but not pre-releases (-...).
    if a[3].startswith("-") or b[3].startswith("-"):
        return False
    return a[:2] == b[:2] and b[2] > a[2]


# --- collecting changes ------------------------------------------------------


def crates(lock_text: str) -> dict[str, set[str]]:
    if not lock_text:
        return {}
    out: dict[str, set[str]] = {}
    for pkg in tomllib.loads(lock_text).get("package", []):
        if str(pkg.get("source", "")).startswith(CRATES_IO_SOURCES):
            out.setdefault(pkg["name"], set()).add(pkg["version"])
    return out


def version_key(v: str) -> tuple[int, int, int, str]:
    return parse_version(v) or (0, 0, 0, v)


def latest(versions: set[str]) -> str | None:
    return max(versions, key=version_key) if versions else None


def patch_base(old_versions: set[str], new: str) -> str | None:
    """The base version that `new` is a patch bump of, if any."""
    for old in sorted(old_versions):
        if is_patch_bump(old, new):
            return old
    return None


def crate_changes(base: str) -> list[Change]:
    old, new = crates(at_base(base, "Cargo.lock")), crates(at_head("Cargo.lock"))
    changes = []
    for name, versions in sorted(new.items()):
        before = old.get(name, set())
        for v in sorted(versions - before):
            prev = patch_base(before, v) or latest(before)
            changes.append(Change("crate", name, prev, v))
    return changes


def toolchain_channel(text: str) -> str | None:
    if not text:
        return None
    return tomllib.loads(text).get("toolchain", {}).get("channel")


def toolchain_changes(base: str) -> tuple[list[Change], list[str]]:
    old = toolchain_channel(at_base(base, "rust-toolchain.toml"))
    new = toolchain_channel(at_head("rust-toolchain.toml"))
    problems = []
    if new is None:
        return [], problems
    if not re.fullmatch(r"\d+\.\d+\.\d+", new):
        problems.append(
            f"rust-toolchain.toml: channel {new!r} is not an exact version (e.g. 1.99.1)"
        )
        return [], problems
    if new == old:
        return [], problems
    return [Change("toolchain", "rust", old, new)], problems


def action_pins(text: str) -> list[tuple[str, str, str | None]]:
    """(action, ref, version comment) for each remote `uses:` line.

    Local actions (`./...`) are skipped. A `uses:` with no `@` comes back with an
    empty ref so the caller reports it as unpinned.
    """
    pins = []
    for m in USES_RE.finditer(text):
        spec, comment = m[1].strip("'\""), m[2]
        if spec.startswith("./"):
            continue
        action, _, ref = spec.rpartition("@") if "@" in spec else (spec, "", "")
        pins.append((action, ref, comment))
    return pins


def action_repo(action: str) -> str:
    return "/".join(action.split("/")[:2])


def action_changes(base: str) -> tuple[list[Change], list[str]]:
    old_pins: dict[str, set[tuple[str, str | None]]] = {}
    for path in files_under(base, ACTION_FILES):
        for action, ref, comment in action_pins(at_base(base, path)):
            old_pins.setdefault(action_repo(action), set()).add((ref, comment))

    changes, problems, seen = [], [], set()
    for path in files_under(None, ACTION_FILES):
        for action, ref, comment in action_pins(at_head(path)):
            if action.startswith("docker://"):
                if not ref.startswith("sha256:"):
                    problems.append(f"{path}: {action} is not pinned by @sha256 digest")
                continue
            repo = action_repo(action)
            if not SHA_RE.match(ref):
                shown = f"{action}@{ref}" if ref else action
                problems.append(f"{path}: {shown} is not pinned to a commit SHA")
                continue
            if not comment:
                problems.append(f"{path}: {action}@{ref[:12]} has no `# vX.Y.Z` comment")
                continue
            if (ref, comment) in old_pins.get(repo, set()) or (repo, ref, comment) in seen:
                continue
            seen.add((repo, ref, comment))
            before = {c for _, c in old_pins.get(repo, set()) if c}
            prev = patch_base(before, comment) or latest(before)
            changes.append(Change("action", repo, prev, comment, note=ref))
    return changes, problems


def justfile_tools(text: str) -> dict[str, str]:
    out = {}
    for m in re.finditer(r'^(\w+)_version\s*:=\s*"([^"]+)"', text, re.MULTILINE):
        if m[1] in JUSTFILE_TOOLS:
            out[JUSTFILE_TOOLS[m[1]]] = m[2]
    return out


def tool_changes(base: str) -> list[Change]:
    old, new = justfile_tools(at_base(base, "justfile")), justfile_tools(at_head("justfile"))
    return [
        Change("tool", name, old.get(name), v)
        for name, v in sorted(new.items())
        if old.get(name) != v
    ]


# --- publish dates -----------------------------------------------------------


def fetch(url: str, token: str | None = None) -> bytes:
    headers = {"User-Agent": USER_AGENT}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, headers=headers)
    attempts = 4
    for attempt in range(attempts):
        last = attempt == attempts - 1
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                return resp.read()
        except urllib.error.HTTPError as e:
            limited = e.code == 429 or (
                e.code == 403 and e.headers.get("X-RateLimit-Remaining") == "0"
            )
            if limited and last and "api.github.com" in url and not token:
                raise RuntimeError("GitHub API rate limit hit; set GH_TOKEN (or GITHUB_TOKEN) and re-run") from e
            if last or not (limited or e.code >= 500):
                raise
            time.sleep(retry_delay(e, attempt))
            continue
        except OSError:
            # URLError, timeouts and connection resets are all OSError.
            if last:
                raise
        time.sleep(2 * (attempt + 1))
    raise AssertionError("unreachable")


def retry_delay(e: urllib.error.HTTPError, attempt: int, cap: float = 60.0) -> float:
    """Seconds to wait before retrying, from Retry-After or X-RateLimit-Reset."""
    if (after := e.headers.get("Retry-After")) and after.isdigit():
        return min(float(after), cap)
    if (reset := e.headers.get("X-RateLimit-Reset")) and reset.isdigit():
        return min(max(float(reset) - time.time(), 1.0), cap)
    return float(2 * (attempt + 1))


def parse_time(s: str) -> dt.datetime:
    t = dt.datetime.fromisoformat(s.replace("Z", "+00:00"))
    return t if t.tzinfo else t.replace(tzinfo=dt.UTC)


_last_crates_io = 0.0


@functools.cache
def crates_io(path: str) -> dict:
    """GET a crates.io API path, at most once per run and once per second."""
    global _last_crates_io
    wait = 1.0 - (time.monotonic() - _last_crates_io)
    if wait > 0:
        time.sleep(wait)
    _last_crates_io = time.monotonic()
    return json.loads(fetch(f"https://crates.io/api/v1/crates/{path}"))


def crate_published(name: str, version: str) -> dt.datetime:
    return parse_time(crates_io(f"{name}/{version}")["version"]["created_at"])


# --- --fix: settle too-new crates on the newest eligible release ---------------


def compatible(current: str, candidate: str) -> bool:
    """Whether `candidate` is in Cargo's semver-compatible range of `current`."""
    a, b = parse_version(current), parse_version(candidate)
    if not a or not b:
        return False
    if a[0] > 0:
        return a[0] == b[0]
    if a[1] > 0:
        return b[0] == 0 and a[1] == b[1]
    return a[:3] == b[:3]


def eligible_version(name: str, current: str, cutoff: dt.datetime) -> str | None:
    """The newest release of `name` that is old enough, not yanked, not a
    pre-release, semver-compatible with `current`, and not newer than it."""
    versions = crates_io(f"{name}/versions?per_page=100")["versions"]
    best = None
    for v in versions:
        num = v["num"]
        parsed = parse_version(num)
        if (
            v["yanked"]
            or not parsed
            or parsed[3].startswith("-")
            or not compatible(current, num)
            or version_key(num) > version_key(current)
            or parse_time(v["created_at"]) > cutoff
        ):
            continue
        if best is None or version_key(num) > version_key(best):
            best = num
    return best


def settle(base: str, min_days: int, now: dt.datetime, rounds: int = 5) -> int:
    """Pins each too-new crate in Cargo.lock to its newest eligible release
    with `cargo update --precise`, repeating while that changes the graph."""
    cutoff = now - dt.timedelta(days=min_days)
    stuck: dict[str, str] = {}
    for _ in range(rounds):
        # A crate stuck behind a sibling's exact pin (pest behind pest_derive,
        # say) can settle once the sibling moves, so retry every round.
        stuck = {}
        changed = False
        for change in crate_changes(base):
            if change.old and is_patch_bump(change.old, change.new):
                continue
            key = f"{change.name}@{change.new}"
            if crate_published(change.name, change.new) <= cutoff:
                continue
            target = eligible_version(change.name, change.new, cutoff)
            if target is None:
                stuck[key] = "no compatible release is old enough"
                continue
            run = subprocess.run(
                ["cargo", "update", "-p", key, "--precise", target],
                capture_output=True,
                text=True,
            )
            if run.returncode != 0:
                reason = run.stderr.strip().splitlines()
                stuck[key] = reason[-1] if reason else "cargo update failed"
                continue
            print(f"pinned {key} -> {target}", flush=True)
            changed = True
        if not changed:
            break
    for key, reason in stuck.items():
        print(f"stuck: {key}: {reason}", flush=True)
    return 1 if stuck else 0


def toolchain_published(version: str) -> dt.datetime:
    text = fetch(f"https://static.rust-lang.org/dist/channel-rust-{version}.toml")
    m = re.search(rb'^date = "(\d{4}-\d{2}-\d{2})"', text, re.MULTILINE)
    if not m:
        raise ValueError(f"no date in the channel manifest for {version}")
    return parse_time(m[1].decode())


def action_published(repo: str, tag: str, sha: str, token: str | None) -> dt.datetime:
    api = f"https://api.github.com/repos/{repo}"
    # The comment must name the tag the SHA comes from.
    tag_sha = json.loads(fetch(f"{api}/commits/{tag}", token))["sha"]
    if tag_sha != sha:
        raise ValueError(f"tag {tag} is {tag_sha[:12]}, not the pinned {sha[:12]}")
    try:
        release = json.loads(fetch(f"{api}/releases/tags/{tag}", token))
    except urllib.error.HTTPError as e:
        if e.code == 404:
            # Commit and tagger dates are set by whoever made them, so they
            # can't prove age. Only a GitHub release date counts.
            raise ValueError(f"{tag} has no GitHub release, so its age can't be confirmed") from e
        raise
    return parse_time(release["published_at"])


# --- checking ----------------------------------------------------------------


def check(change: Change, min_days: int, now: dt.datetime, token: str | None) -> Result:
    if change.kind != "action" and change.old and is_patch_bump(change.old, change.new):
        return Result(change, "patch", f"patch bump of {change.old}")
    try:
        if change.kind in ("crate", "tool"):
            published = crate_published(change.name, change.new)
        elif change.kind == "toolchain":
            published = toolchain_published(change.new)
        else:
            published = action_published(change.name, change.new, change.note, token)
    except Exception as e:  # noqa: BLE001 - any failure means "not verified"
        return Result(change, "error", f"could not verify: {e}")
    age = now - published
    ready = published + dt.timedelta(days=min_days)
    days = f"{age.days} day{'' if age.days == 1 else 's'}"
    detail = f"published {published:%Y-%m-%d}, {days} ago"
    if age < dt.timedelta(days=min_days):
        return Result(change, "too-new", f"{detail}; eligible {ready:%Y-%m-%d}")
    return Result(change, "ok", detail)


def report(results: list[Result], problems: list[str], min_days: int, exempt: bool) -> str:
    lines = [f"## Dependency age (minimum {min_days} days)", ""]
    if results:
        lines += ["| Status | Kind | Name | Change | Detail |", "| --- | --- | --- | --- | --- |"]
        icon = {"ok": "pass", "patch": "pass (patch)", "too-new": "FAIL", "error": "FAIL"}
        for r in results:
            c = r.change
            change = f"{c.old} → {c.new}" if c.old else f"new {c.new}"
            lines.append(f"| {icon[r.status]} | {c.kind} | `{c.name}` | {change} | {r.detail} |")
    else:
        lines.append("No new or changed dependency versions.")
    if problems:
        lines += ["", "**Pinning problems:**", *[f"- {p}" for p in problems]]
    if exempt:
        lines += ["", "**`dependency-age-exempt` is set: \"too new\" is reported but doesn't fail. Pinning problems and unverified versions still fail.**"]
    return "\n".join(lines) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--base", help="revision to compare with (default: merge-base with origin/main)")
    ap.add_argument("--min-days", type=int, default=14)
    ap.add_argument("--exempt", action="store_true", help="report failures without failing")
    ap.add_argument(
        "--fix",
        action="store_true",
        help="pin too-new crates in Cargo.lock to their newest eligible release, then check",
    )
    args = ap.parse_args()

    # Paths below are relative to the repo root, whatever the current directory.
    os.chdir(git("rev-parse", "--show-toplevel").strip())
    if args.base:
        base = args.base
    else:
        try:
            base = git("merge-base", "HEAD", "origin/main").strip()
        except subprocess.CalledProcessError:
            sys.exit("error: no merge-base with origin/main; run `git fetch origin` or pass --base")
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    now = dt.datetime.now(dt.UTC)
    if args.fix and settle(base, args.min_days, now):
        print("note: some crates couldn't be settled; see `stuck:` above", file=sys.stderr)

    changes = crate_changes(base) + tool_changes(base)
    tc, tc_problems = toolchain_changes(base)
    ac, ac_problems = action_changes(base)
    changes += tc + ac
    problems = list(dict.fromkeys(tc_problems + ac_problems))  # dedupe, keep order

    results = [check(c, args.min_days, now, token) for c in changes]
    text = report(results, problems, args.min_days, args.exempt)
    print(text, flush=True)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a") as f:
            f.write(text)

    hard_fail = bool(problems) or any(r.status == "error" for r in results)
    too_new = any(r.status == "too-new" for r in results)
    if hard_fail or (too_new and not args.exempt):
        print("error: dependency age check failed (see CONTRIBUTING.md, 'The 14-day rule')", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
