#!/usr/bin/env python3
"""Install or restore the local macOS/Bun Codex theme fix, preserving its package."""

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
    state = root / "local-theme-fix.json"
    if args.restore:
        record = json.loads(state.read_text())
        backup = Path(record["backup"])
        if digest(binary) != record["patched_sha256"] or digest(backup) != record["original_sha256"]:
            raise SystemExit("Binary or backup changed; refusing to overwrite it.")
        source = backup
    else:
        source = Path(__file__).resolve().parents[1] / "codex-rs/target/dev-small/codex"
        version = json.loads((root / "codex-package.json").read_text())["version"]
        if version != "0.166.0":
            raise SystemExit("This patch was tested against 0.166.0; revalidate before installing over another release.")
        if state.exists():
            raise SystemExit("Local patch already installed; restore it before reinstalling.")
        backup = binary.with_name(f"codex.upstream-{version}")
        if backup.exists():
            if digest(backup) != digest(binary):
                raise SystemExit("Existing upstream backup differs; refusing to overwrite it.")
        else:
            shutil.copy2(binary, backup)
        record = {"backup": str(backup), "original_sha256": digest(backup), "patched_sha256": digest(source)}
    staged = binary.with_name("codex.theme-fix-staged")
    shutil.copy2(source, staged)
    os.replace(staged, binary)
    if args.restore:
        state.unlink()
    else:
        state.write_text(json.dumps(record, indent=2) + "\n")
    print(f"{'Restored upstream' if args.restore else 'Installed local theme fix'}: {binary}")
    print(f"Upstream backup: {backup}")


if __name__ == "__main__":
    main()
