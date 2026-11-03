#!/usr/bin/env python3
"""Explicit local CLI installation. Python is needed only by this installer."""
import argparse
import fcntl
import hashlib
import json
import os
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import uuid

LAUNCHER = b'''#!/bin/sh
set -eu
base=$(CDPATH= cd -- "$(dirname -- "$0")/../lib/macscan/current" 2>/dev/null && pwd -P) || {
  echo '{"schema_version":"3.0","type":"error","status":"failed","outcome":"failed","error":{"code":"IO_ERROR","operation":"runtime_init","detail":"Installation incomplete; reinstall the CLI","retryable":false}}' >&2; exit 3;
}
if [ ! -f "$base/libduckdb.dylib" ]; then
  echo '{"schema_version":"3.0","type":"error","status":"failed","outcome":"failed","error":{"code":"IO_ERROR","operation":"runtime_init","detail":"Bundled libduckdb.dylib missing; reinstall the CLI","retryable":false}}' >&2; exit 3
fi
if [ ! -x "$base/macscan" ]; then
  echo '{"schema_version":"3.0","type":"error","status":"failed","outcome":"failed","error":{"code":"IO_ERROR","operation":"runtime_init","detail":"CLI payload missing or not executable; reinstall the CLI","retryable":false}}' >&2; exit 3
fi
exec "$base/macscan" "$@"
'''
FILES = {'macscan', 'libduckdb.dylib', 'manifest.json'}

def run(*args, env=None):
    p = subprocess.run(args, capture_output=True, text=True, env=env)
    if p.returncode:
        raise ValueError(f"{args[0]} failed: {p.stderr.strip()[:600]}")
    return p

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def regular(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"Expected regular owned file: {path}")

def safe_directory(path):
    for item in [path, *path.parents]:
        if item.is_symlink() or (item.exists() and not item.is_dir()):
            raise ValueError(f"Refusing non-directory or symlink: {item}")

def identity(binary, rpath):
    regular(binary)
    run('/usr/bin/codesign', '--verify', '--strict', str(binary))
    lines = run('/usr/bin/otool', '-l', str(binary)).stdout.splitlines()
    paths = [lines[i+2].strip().split(' (offset')[0].removeprefix('path ')
             for i, line in enumerate(lines) if line.strip() == 'cmd LC_RPATH']
    if paths != [rpath]:
        raise ValueError(f"Packaged runtime paths must be exactly {rpath}: {paths}")
    env = dict(os.environ)
    env.pop('HOME', None)
    # Never allow a user's loader override to make artifact validation pass.
    for key in list(env):
        if key.startswith('DYLD_'):
            del env[key]
    value = json.loads(run(str(binary), '--version', env=env).stdout)['identity']
    if value['build_revision'] == 'unknown' or value['build_revision'].endswith('-dirty'):
        raise ValueError('Install requires an identified clean release build')
    return value

def release_manifest(release, retiring=False):
    if release.is_symlink() or not release.is_dir():
        raise ValueError("Foreign release directory")
    names = {p.name for p in release.iterdir()}
    if retiring and not names:
        return {}
    if (not retiring and names != FILES) or (retiring and (not names <= FILES or "manifest.json" not in names)):
        raise ValueError('Unknown or missing release files; refusing to modify')
    for name in names:
        regular(release / name)
    manifest = json.loads((release / 'manifest.json').read_text())
    if manifest.get('owner') != 'macscan-cli-1' or set(manifest.get('sha256', {})) != FILES - {'manifest.json'}:
        raise ValueError('Foreign installation manifest')
    for name, expected in manifest['sha256'].items():
        if name in names and digest(release / name) != expected:
            raise ValueError(f'Changed owned file: {name}; refusing to modify')
    return manifest

def retire(release):
    # The manifest is removed last, allowing safe recovery after partial cleanup.
    for name in ('macscan', 'libduckdb.dylib', 'manifest.json'):
        (release / name).unlink(missing_ok=True)
    release.rmdir()

def installed(root, launcher):
    if root.is_symlink() or not root.is_dir():
        raise ValueError(f"Foreign installation: {root}")
    current = root / 'current'
    if not current.is_symlink():
        raise ValueError('Missing owned current release link')
    target = os.readlink(current)
    if not re.fullmatch(r'release-[a-f0-9]{32}', target):
        raise ValueError('Foreign current release target')
    release = root / target
    manifest = release_manifest(release)
    actual = identity(release / 'macscan', '@executable_path')
    if actual != manifest.get('identity') or actual != manifest.get('app_identity'):
        raise ValueError('Installed identity metadata drift; refusing to modify')
    previous = manifest.get('previous_releases', [])
    if not isinstance(previous, list) or any(not isinstance(n, str) or not re.fullmatch(r'release-[a-f0-9]{32}', n) for n in previous):
        raise ValueError('Foreign previous release declaration')
    names = {p.name for p in root.iterdir()}
    if not names <= {'current', target, *previous}:
        raise ValueError('Unknown files in installation; refusing to modify')
    for name in names - {'current', target}:
        release_manifest(root / name, retiring=True)
    regular(launcher)
    if launcher.read_bytes() != LAUNCHER:
        raise ValueError('Foreign bin/macscan; refusing to modify')
    return release, manifest

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['install', 'uninstall', 'status'])
    parser.add_argument('--prefix', required=True, type=Path, help='explicit absolute prefix; bin/macscan is created here')
    parser.add_argument('--artifacts', type=Path, help='built dist directory; required for install/update')
    parser.add_argument('--app', type=Path, help='matching newly built Macscan.app; required for install/update, never /Applications')
    args = parser.parse_args()
    prefix = args.prefix
    if not prefix.is_absolute() or '..' in prefix.parts:
        raise ValueError('--prefix must be absolute without parent components')
    safe_directory(prefix)
    if args.action == 'install':
        prefix.mkdir(parents=True, exist_ok=True)
    # A directory lock has no persistent lock file and serializes this prefix only.
    fd = os.open(prefix, os.O_RDONLY)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        perform(args)
    finally:
        os.close(fd)

def perform(args):
    prefix = args.prefix
    if not prefix.is_absolute() or '..' in prefix.parts:
        raise ValueError('--prefix must be absolute without parent components')
    safe_directory(prefix)
    for directory in [prefix / 'bin', prefix / 'lib']:
        safe_directory(directory)
    root, launcher = prefix / 'lib/macscan', prefix / 'bin/macscan'
    previous = None
    if root.exists() or root.is_symlink():
        previous = installed(root, launcher)
    elif launcher.exists() or launcher.is_symlink():
        raise ValueError('Foreign bin/macscan collision')
    if args.action in ('status', 'uninstall'):
        if previous is None:
            raise ValueError('No owned CLI installation at this prefix')
        if args.action == 'uninstall':
            # Only known, hash-validated owned files; never inspect a history store.
            launcher.unlink()
            shutil.rmtree(root)
        print(json.dumps({'action': args.action, 'prefix': str(prefix), 'identity': previous[1]['identity'], 'app_identity': previous[1]['app_identity']}))
        return
    if args.artifacts is None or args.app is None:
        raise ValueError('install/update requires --artifacts and --app for release coherence')
    source = args.artifacts.absolute()
    app = args.app.absolute()
    if str(app).startswith('/Applications/'):
        raise ValueError('Supply the new built app artifact, not an installed GUI app')
    cli_identity = identity(source / 'macscan', '@executable_path')
    run('/usr/bin/codesign', '--verify', '--deep', '--strict', str(app))
    app_identity = identity(app / 'Contents/MacOS/macscan-desktop', '@executable_path/../Frameworks')
    if cli_identity != app_identity:
        raise ValueError('CLI/app version drift; build both from the same clean commit')
    library = source / 'libduckdb.dylib'
    regular(library)
    run('/usr/bin/codesign', '--verify', '--strict', str(library))
    app_library = app / 'Contents/Frameworks/libduckdb.dylib'
    regular(app_library)
    if digest(library) != digest(app_library):
        raise ValueError('CLI/app DuckDB runtime drift')
    (prefix / 'lib').mkdir(parents=True, exist_ok=True)
    (prefix / 'bin').mkdir(exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix='.macscan-stage-', dir=prefix / 'lib'))
    new_launcher = None
    try:
        release_name = 'release-' + uuid.uuid4().hex
        release = staging / release_name
        release.mkdir()
        for name in ['macscan', 'libduckdb.dylib']:
            shutil.copy2(source / name, release / name)
        staged_identity = identity(release / 'macscan', '@executable_path')
        if staged_identity != cli_identity:
            raise ValueError('Staged identity changed')
        run('/usr/bin/codesign', '--verify', '--strict', str(release / 'libduckdb.dylib'))
        manifest = {'owner': 'macscan-cli-1', 'identity': cli_identity, 'app_identity': app_identity,
                    'previous_releases': sorted(p.name for p in root.iterdir() if p.name != 'current') if previous else [],
                    'sha256': {name: digest(release / name) for name in ['macscan', 'libduckdb.dylib']}}
        (release / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (staging / 'current').symlink_to(release_name)
        if previous:
            # Recheck immediately before activation; previous current remains valid on staging failure.
            installed(root, launcher)
            os.rename(release, root / release_name)
            try:
                os.replace(staging / 'current', root / 'current')
            except BaseException:
                shutil.rmtree(root / release_name)
                raise
            # Activation is committed. Cleanup cannot retroactively report failure.
            for name in manifest['previous_releases']:
                try:
                    retire(root / name)
                except OSError as error:
                    print(f'Warning: update active; retained validated previous payload {name}: {error}', file=sys.stderr)
        else:
            fd, name = tempfile.mkstemp(prefix='.macscan-launcher-', dir=prefix / 'bin')
            new_launcher = Path(name)
            with os.fdopen(fd, 'wb') as stream:
                stream.write(LAUNCHER)
            new_launcher.chmod(0o755)
            os.rename(staging, root)
            try:
                # Exclusive link prevents clobbering a concurrent foreign launcher.
                os.link(new_launcher, launcher)
            except BaseException:
                shutil.rmtree(root)
                raise
        print(json.dumps({'action': 'updated' if previous else 'installed', 'prefix': str(prefix), 'command': str(launcher), 'identity': cli_identity, 'app_identity': app_identity}))
        if str(prefix / 'bin') not in os.environ.get('PATH', '').split(os.pathsep):
            print('PATH does not contain this bin directory. For this shell run:\nexport PATH=' + shlex.quote(str(prefix / 'bin')) + ':"$PATH"', file=sys.stderr)
    finally:
        # Cleanup failures after activation are warnings, never false rollback claims.
        try:
            if staging.exists():
                shutil.rmtree(staging)
            if new_launcher is not None:
                new_launcher.unlink(missing_ok=True)
        except OSError as error:
            print(f'Warning: temporary installer cleanup incomplete: {error}', file=sys.stderr)

if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, json.JSONDecodeError) as error:
        print(f'macscan installer: {error}', file=sys.stderr)
        sys.exit(3)
