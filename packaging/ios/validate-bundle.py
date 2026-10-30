#!/usr/bin/env python3
"""Verify an Xcode bundle has a real executable and, by default, valid signing."""
import argparse
from pathlib import Path
import plistlib
import subprocess


def validate(bundle, require_signature=True):
    with (bundle / 'Info.plist').open('rb') as source:
        info = plistlib.load(source)
    name = info.get('CFBundleExecutable')
    if not isinstance(name, str) or Path(name).name != name:
        raise ValueError('CFBundleExecutable must name a file in the bundle')
    executable = bundle / name
    if not executable.is_file():
        raise ValueError(f'Missing bundle executable: {executable}')
    with executable.open('rb') as source:
        magic = source.read(4)
    if magic not in {b'\xcf\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca', b'\xca\xfe\xba\xbf', b'\xbf\xba\xfe\xca'}:
        raise ValueError(f'Bundle executable is not Mach-O: {executable}')
    # Inspect each architecture's Mach header, not just an archive or dylib magic.
    result = subprocess.run(['xcrun', 'otool', '-hv', str(executable)], check=True, capture_output=True, text=True)
    if 'EXECUTE' not in result.stdout:
        raise ValueError('Mach-O has no executable header')
    if require_signature:
        subprocess.run(['codesign', '--verify', '--strict', '--verbose=2', str(bundle)], check=True)
    return executable


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path)
    parser.add_argument('--unsigned', action='store_true', help='Skip signing check for CODE_SIGNING_ALLOWED=NO builds')
    arguments = parser.parse_args()
    print(validate(arguments.bundle, not arguments.unsigned))
