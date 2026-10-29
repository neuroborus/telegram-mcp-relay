#!/usr/bin/env python3
"""Check portable repository navigation, canonical skills and Runner setup."""

from __future__ import annotations

import json
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[1]


def git(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(ROOT), *args], capture_output=True, text=True, check=False
    )


def main() -> int:
    errors: list[str] = []
    inventory = git("ls-files", "-z", "--cached", "--others", "--exclude-standard")
    if inventory.returncode:
        print("Cannot inspect repository inventory.", file=sys.stderr)
        return 1
    files = sorted(set(filter(None, inventory.stdout.split("\0"))))
    for relative in files:
        path = ROOT / relative
        if not path.exists():
            # A tracked deletion is a valid workspace change.
            if path.is_symlink():
                errors.append(f"{relative}: broken symlink")
            continue
        if path.is_symlink() or path.suffix != ".md":
            continue
        text = path.read_text(encoding="utf-8")
        without_fences = re.sub(r"(?ms)^```.*?^```[^\n]*", "", text)
        for target in re.findall(r"\[[^\]\n]*\]\(([^)\n]+)\)", without_fences):
            target = target.strip().split(' "', 1)[0].strip("<>")
            parsed = urlsplit(target)
            if parsed.scheme or parsed.netloc or not parsed.path:
                continue
            destination = (path.parent / unquote(parsed.path)).resolve()
            if not destination.is_relative_to(ROOT) or not destination.exists():
                errors.append(f"{relative}: missing or unconfined link {target}")

    skills = ROOT / ".agents/skills"
    for folder in sorted(skills.iterdir()):
        if not folder.is_dir():
            continue
        entry = folder / "SKILL.md"
        if not entry.is_file():
            errors.append(f"{folder.relative_to(ROOT)}: missing SKILL.md")
            continue
        text = entry.read_text(encoding="utf-8")
        match = re.match(r"\A---\n(.*?)\n---(?:\n|$)", text, re.S)
        if not match:
            errors.append(f"{entry.relative_to(ROOT)}: missing YAML frontmatter")
            continue
        fields = match.group(1)
        name = re.search(r"^name:\s*([^\n]+)$", fields, re.M)
        description = re.search(r"^description:\s*([^\n]+)$", fields, re.M)
        if not name or name.group(1).strip() != folder.name:
            errors.append(f"{entry.relative_to(ROOT)}: name must match folder")
        if not description or not description.group(1).strip():
            errors.append(f"{entry.relative_to(ROOT)}: missing description")

    for relative, expected in [
        ("CLAUDE.md", "AGENTS.md"),
        (".claude/skills", ".agents/skills"),
    ]:
        path = ROOT / relative
        if not path.is_symlink() or path.resolve() != (ROOT / expected).resolve():
            errors.append(f"{relative}: must be a symlink to {expected}")

    try:
        config = json.loads((ROOT / "agent-runner.example.json").read_text())
        if config["schemaVersion"] != 1 or config["artifactRoot"] != "LOCAL_ARTIFACTS":
            errors.append("agent-runner.example.json: invalid schema/artifact root")
        for pipeline in ("plan-authoring", "plan-execution", "polishing"):
            if config["pipelines"][pipeline]["mode"] != "independent":
                errors.append(f"agent-runner.example.json: {pipeline} must be independent")
        for pipeline in ("plan-execution", "polishing"):
            if config["pipelines"][pipeline]["finalization"] != "auto":
                errors.append(f"agent-runner.example.json: {pipeline} must discover finalization")
    except (OSError, ValueError, KeyError, TypeError) as error:
        errors.append(f"agent-runner.example.json: {error}")

    if git("check-ignore", "-q", "LOCAL_ARTIFACTS/probe").returncode:
        errors.append("LOCAL_ARTIFACTS/ must be ignored")
    if git("ls-files", "--", "LOCAL_ARTIFACTS").stdout:
        errors.append("LOCAL_ARTIFACTS/ contains tracked files")

    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Repository checks passed ({len(files)} files, {len(list(skills.iterdir()))} skills).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
