#!/usr/bin/env python3
"""Sign and package a release binary for the Green star GitHub installer."""
import hashlib
import json
import pathlib
import struct
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[1]
binary = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "target/release/green-star-pdf-reader"
manifest = json.loads((root / "manifest.json").read_text())
subprocess.run(["codesign", "--force", "--sign", "-", "--timestamp=none", str(binary)], check=True)
subprocess.run(["codesign", "--verify", "--strict", str(binary)], check=True)
parts = [json.dumps(manifest, separators=(",", ":")).encode(), binary.read_bytes()]
if any(len(part) > 64 * 1024**2 for part in parts):
    raise SystemExit("Package frame exceeds the installer limit")
data = b"".join(struct.pack("<I", len(part)) + part for part in parts)
destination = root / "dist"
destination.mkdir(exist_ok=True)
package = destination / "viewer-aarch64-apple-darwin.zgv"
package.write_bytes(data)
checksum = package.with_suffix(".zgv.sha256")
checksum.write_text(hashlib.sha256(data).hexdigest() + "  " + package.name + "\n")
print(package)
print(checksum)
