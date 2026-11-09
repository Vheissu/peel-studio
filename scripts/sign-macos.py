#!/usr/bin/env python3
"""Sign bundled native libraries before signing the enclosing macOS app."""
import os
from pathlib import Path
import subprocess
import sys

MACH_O = {bytes.fromhex(magic) for magic in (
    "feedface", "feedfacf", "cefaedfe", "cffaedfe", "cafebabe", "bebafeca", "cafebabf", "bfbafeca")}


def sign(app):
    identity = os.environ.get("SIGN_ID", "-")
    # Ad-hoc signatures have no Team ID, so hardened library validation rejects
    # even the interpreter's own dylib. Developer ID builds keep hardened runtime.
    options = ["--options", "runtime"] if identity != "-" else []
    timestamp = "--timestamp" if identity != "-" else "--timestamp=none"
    native = []
    for path in app.rglob("*"):
        if path.is_symlink() or not path.is_file():
            continue
        with path.open("rb") as source:
            if source.read(4) in MACH_O:
                native.append(path)
    # Libraries and Python extension modules first; enclosing bundles last.
    for path in sorted(native, key=lambda p: (-len(p.parts), str(p))):
        subprocess.run(["codesign", "--force", *options, timestamp,
                        "-s", identity, str(path)], check=True)
    subprocess.run(["codesign", "--force", *options, timestamp,
                    "-s", identity, str(app)], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", "--verbose=1", str(app)], check=True)


if __name__ == "__main__":
    sign(Path(sys.argv[1]))
