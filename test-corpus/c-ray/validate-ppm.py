#!/usr/bin/env python3

"""Validate C-Ray's exact binary PPM contract and optionally compare it."""

import argparse
import hashlib
from pathlib import Path
import sys


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--path", required=True, type=Path)
    parser.add_argument("--width", required=True, type=int)
    parser.add_argument("--height", required=True, type=int)
    parser.add_argument("--compare", type=Path)
    arguments = parser.parse_args()
    if arguments.width <= 0 or arguments.height <= 0:
        parser.error("image dimensions must be positive")
    return arguments


def main() -> int:
    arguments = parse_arguments()
    image = arguments.path.read_bytes()
    header = f"P6\n{arguments.width} {arguments.height}\n255\n".encode("ascii")
    expected_size = len(header) + arguments.width * arguments.height * 3
    if not image.startswith(header):
        print(f"{arguments.path}: invalid C-Ray P6 header", file=sys.stderr)
        return 1
    if len(image) != expected_size:
        print(
            f"{arguments.path}: expected {expected_size} bytes, found {len(image)}",
            file=sys.stderr,
        )
        return 1
    if arguments.compare is not None:
        reference = arguments.compare.read_bytes()
        if image != reference:
            print(
                f"{arguments.path}: image differs from {arguments.compare}",
                file=sys.stderr,
            )
            return 1
    print(hashlib.sha256(image).hexdigest())
    return 0


if __name__ == "__main__":
    sys.exit(main())
