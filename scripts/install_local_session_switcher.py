#!/usr/bin/env python3
"""Install/restore the session-switcher build without losing the previous custom build."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--restore", action="store_true")
    args = parser.parse_args()
    root = Path.home() / ".bun/install/global/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin"
    binary = root / "bin/codex"
    state = root / "local-session-switcher.json"
    theme_state = root / "local-theme-fix.json"
    theme_record = json.loads(theme_state.read_text())
    if digest(binary) != theme_record["patched_sha256"]:
        raise SystemExit("Installed binary differs from the custom-build record; refusing to replace it.")
    if args.restore:
        record = json.loads(state.read_text())
        source = Path(record["backup"])
        if digest(binary) != record["patched_sha256"] or digest(source) != record["previous_sha256"]:
            raise SystemExit("Build or backup changed; refusing to overwrite it.")
    else:
        if state.exists():
            raise SystemExit("Session switcher already installed; restore before reinstalling.")
        source = Path(__file__).resolve().parents[1] / "codex-rs/target/dev-small/codex"
        patched_sha = digest(source)
        previous_sha = digest(binary)
        backup = binary.with_name("codex.before-session-switcher")
        if backup.exists() and digest(backup) != previous_sha:
            raise SystemExit("A different previous-build backup already exists.")
        if not backup.exists():
            shutil.copy2(binary, backup)
        record = {"backup": str(backup), "previous_sha256": previous_sha, "patched_sha256": patched_sha}
    staged = binary.with_name("codex.switcher-staged")
    shutil.copy2(source, staged)
    os.replace(staged, binary)
    # Keep the earlier restore-to-official command valid as well.
    theme_record["patched_sha256"] = digest(binary)
    theme_state.write_text(json.dumps(theme_record, indent=2) + "\n")
    if args.restore:
        state.unlink()
    else:
        state.write_text(json.dumps(record, indent=2) + "\n")
    print("Restored previous custom build." if args.restore else "Installed session switcher; previous custom build backed up.")
    print(binary)


if __name__ == "__main__":
    main()
