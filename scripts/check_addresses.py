#!/usr/bin/env python3
"""Fail CI if artifacts/addresses.json disagrees with README's Deployed
addresses table. The README is the source of truth (it's hand-verified
against `stellar contract info interface` at redeploy time); this script
only checks that the generated artifact hasn't been left stale."""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
README = ROOT / "README.md"
ADDRESSES_JSON = ROOT / "artifacts" / "addresses.json"


def parse_readme_table(text: str) -> dict[str, str]:
    section = re.search(
        r"## Deployed addresses.*?\n(.*?)(?=\n## |\Z)", text, re.DOTALL
    )
    if not section:
        print("::error::README.md has no 'Deployed addresses' section")
        sys.exit(1)

    rows = re.findall(
        r"^\|\s*([A-Za-z]+)\s*\|\s*`([A-Z0-9]+)`\s*\|$", section.group(1), re.MULTILINE
    )
    if not rows:
        print("::error::no contract rows found in README's Deployed addresses table")
        sys.exit(1)

    return {name.strip().lower(): addr for name, addr in rows}


def main() -> None:
    if not ADDRESSES_JSON.exists():
        print(f"::error::{ADDRESSES_JSON} does not exist")
        sys.exit(1)

    readme_addrs = parse_readme_table(README.read_text())
    artifact = json.loads(ADDRESSES_JSON.read_text())
    artifact_addrs = {k.lower(): v for k, v in artifact.get("contracts", {}).items()}

    mismatches = []
    for name, readme_addr in readme_addrs.items():
        artifact_addr = artifact_addrs.get(name)
        if artifact_addr != readme_addr:
            mismatches.append(f"  {name}: README has {readme_addr}, artifacts/addresses.json has {artifact_addr}")

    missing = set(readme_addrs) - set(artifact_addrs)
    extra = set(artifact_addrs) - set(readme_addrs)

    if mismatches or missing or extra:
        print("::error::artifacts/addresses.json disagrees with README's Deployed addresses table")
        for line in mismatches:
            print(line)
        for name in missing:
            print(f"  {name}: in README but missing from artifacts/addresses.json")
        for name in extra:
            print(f"  {name}: in artifacts/addresses.json but not in README")
        sys.exit(1)

    print(f"OK: {len(readme_addrs)} contract addresses match between README and artifacts/addresses.json")


if __name__ == "__main__":
    main()
