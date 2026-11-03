#!/usr/bin/env python3
"""Owned-prefix acceptance only. Requires final clean CLI/app artifacts and release whole_mac host."""
import argparse
import contextlib
import io
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest.mock

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--artifacts', required=True, type=Path)
p.add_argument('--whole-mac-host', required=True, type=Path)
a = p.parse_args()
sys.dont_write_bytecode = True
artifacts = a.artifacts.resolve()
host = a.whole_mac_host.resolve()
installer = Path(__file__).resolve().with_name('install-cli.py')

def tree(path):
    return {str(f.relative_to(path)): hashlib.sha256(f.read_bytes()).hexdigest()
            for f in path.rglob('*') if f.is_file()} if path.exists() else {}

with tempfile.TemporaryDirectory(prefix='macscan installed acceptance ') as temp:
    base = Path(temp).resolve()
    prefix, store, cwd = base / 'chosen prefix', base / 'history', base / 'another cwd'
    cwd.mkdir()
    env = dict(os.environ, PATH=str(prefix / 'bin') + ':/usr/bin:/bin')
    args = [str(installer), 'install', '--prefix', str(prefix), '--artifacts', str(artifacts), '--app', str(artifacts / 'Macscan.app')]
    def install(arguments=args, expected=0):
        out = subprocess.run([sys.executable, *arguments], capture_output=True, text=True, cwd=cwd)
        assert out.returncode == expected, out.stderr
        return out
    def cli(*args, code=0, where=store, extra_env=None):
        command = ['macscan', *map(str, args)]
        if args[0] not in ('schema', '--version', 'capabilities') and '--help' not in args:
            command += ['--store', str(where)]
        out = subprocess.run(command, cwd=cwd, env=extra_env or env, capture_output=True)
        assert out.returncode == code, (command, out.returncode, out.stdout, out.stderr)
        assert len(out.stdout) <= 8192
        lines = [json.loads(line) for line in out.stderr.splitlines()]
        assert all(len(json.dumps(line).encode()) < 8192 for line in lines)
        if code not in (0, 10):
            assert not out.stdout and lines[-1]['outcome'] in ('failed', 'cancelled')
            return lines[-1]
        assert all(line['type'] == 'progress' for line in lines)
        if '--format' in args and args[args.index('--format') + 1] == 'ndjson':
            rows = [json.loads(line) for line in out.stdout.splitlines()]
            assert rows[0]['type'] == 'start' and rows[-1]['type'] == 'end'
            return rows
        return json.loads(out.stdout)

    assert not store.exists()
    initial = install()
    assert 'PATH does not contain' in initial.stderr
    assert not store.exists()
    version = cli('--version')['identity']
    assert version == json.loads(initial.stdout)['app_identity']
    cli('schema', '--command', 'scan')
    cli('scan', '--help')
    assert cli('scans')['records'] == [] and not store.exists()
    root = base / 'own files'
    root.mkdir()
    for n in range(6):
        (root / f'file{n}.txt').write_bytes(b'owned fixture' * 20)
    first = cli('scan', root, '--save', '--progress', 'ndjson')['scan_id']
    runs = cli('scans')['records']
    assert len(runs) == 1
    members = cli('scans', '--run', runs[0]['run_id'])['records']
    assert members[0]['snapshot_id'] == first
    for command in ('summary', 'insights', 'coverage'):
        cli(command, '--scan', first)
    cli('summary', '--scan', first, '--format', 'ndjson')
    (root / 'file0.txt').write_bytes(b'grown owned fixture' * 1000)
    second = cli('scan', root, '--save')['scan_id']
    cli('growth', first, second)
    cli('scan', root, '--save')
    fourth = cli('scan', root, '--save')
    assert fourth['retention']['removed_summary'] == [first], fourth
    assert fourth['retention']['removed_snapshot_summary'] == [first]
    assert len(cli('scans')['records']) == 3
    cli('summary', '--scan', first, code=4)
    cli('scan', root, '--save', '--max-entries', '2', code=10, where=base / 'partial')
    cli('scan', root, '--save', '--metadata-budget', '1', code=5)
    lock = os.open(store, os.O_RDONLY)
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        assert cli('scan', root, '--save', code=7)['error']['retryable']
    finally:
        os.close(lock)
    # A real installed process, actual SIGINT, owned metadata only.
    slow = base / 'cancel files'
    slow.mkdir()
    for n in range(18000):
        (slow / str(n)).touch()
    child = subprocess.Popen(['macscan', 'scan', str(slow), '--save', '--store', str(base / 'cancel history'), '--progress', 'ndjson'], cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        progress = child.stderr.readline()
        assert json.loads(progress)['type'] == 'progress'
        child.send_signal(signal.SIGINT)
        stdout, stderr = child.communicate(timeout=30)
        assert child.returncode == 130 and not stdout, (child.returncode, stdout, stderr)
        assert json.loads(stderr.splitlines()[-1])['outcome'] == 'cancelled'
        assert len(stderr.splitlines()[-1]) <= 8192
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
    # Fixture creation is deliberately outside the installed binary; no machine inventory.
    grouped = base / 'grouped'
    grouped.mkdir()
    for name in ('a', 'b'):
        (grouped / name).mkdir()
        (grouped / name / 'owned').write_text('synthetic volume')
    group_store = base / 'group history'
    group_runs = []
    for n in range(4):
        out = subprocess.run([str(host), '--host', 'full', str(grouped), 'scan', '--whole-mac', '--save', '--store', str(group_store)], cwd=cwd, capture_output=True)
        assert out.returncode == 0, out.stderr
        receipt = json.loads(out.stdout)
        assert len(out.stdout) <= 8192
        group_runs.append(receipt['run_id'])
        members = cli('scans', '--run', receipt['run_id'], where=group_store)['records']
        assert len(members) == 2
        for row in members:
            for command in ('summary', 'insights', 'coverage'):
                cli(command, '--scan', row['snapshot_id'], where=group_store)
        assert cli('summary', '--scan', receipt['run_id'], code=2, where=group_store)['error']['code'] == 'WRONG_ID_TYPE'
    assert receipt['retention']['removed_summary'] == [group_runs[0]]
    assert len(cli('scans', where=group_store)['records']) == 3
    cli('scans', '--run', group_runs[0], code=4, where=group_store)
    # Loader evidence proves bundled runtime, independent of repository cwd and PATH.
    loaded = subprocess.run([str(prefix / 'lib/macscan/current/macscan'), '--version'], cwd=cwd, env=dict(env, DYLD_PRINT_LIBRARIES='1'), capture_output=True)
    assert loaded.returncode == 0
    dylib_lines = [line for line in loaded.stderr.decode().splitlines() if 'libduckdb.dylib' in line]
    assert len(dylib_lines) == 1 and str(prefix / 'lib/macscan') in dylib_lines[0], dylib_lines
    assert '.build' not in dylib_lines[0]
    before_history = tree(store)
    before_install = tree(prefix)
    # Same-release update validates and activates a fresh, copied payload.
    install()
    assert cli('--version')['identity'] == version and tree(store) == before_history
    before_install = tree(prefix)
    spec = importlib.util.spec_from_file_location('installer', installer)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    original_identity = module.identity
    # Post-activation cleanup failure is success with an explicit warning; a later
    # status/update validates and removes only the recorded previous payload.
    def failing_cleanup(path):
        # Simulate an actual interruption after one known payload file was deleted.
        (Path(path) / 'macscan').unlink()
        raise OSError('owned test: partial cleanup failure after activation')
    warnings = io.StringIO()
    with unittest.mock.patch.object(sys, 'argv', ['installer', *args[1:]]), unittest.mock.patch.object(module, 'retire', failing_cleanup), contextlib.redirect_stderr(warnings):
        module.main()
    assert 'update active; retained validated previous payload' in warnings.getvalue()
    assert cli('--version')['identity'] == version
    install([str(installer), 'status', '--prefix', str(prefix)])
    install()
    before_install = tree(prefix)
    prefix_lock = os.open(prefix, os.O_RDONLY)
    try:
        fcntl.flock(prefix_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        install(expected=3)
    finally:
        os.close(prefix_lock)
    assert tree(prefix) == before_install
    def failing_stage(binary, rpath):
        if '.macscan-stage-' in str(binary):
            raise OSError('owned test: staged validation failure')
        return original_identity(binary, rpath)
    with unittest.mock.patch.object(sys, 'argv', ['installer', *args[1:]]), unittest.mock.patch.object(module, 'identity', failing_stage):
        try:
            module.main()
            raise AssertionError('stage failure accepted')
        except OSError as error:
            assert 'staged validation failure' in str(error)
    assert tree(prefix) == before_install and cli('--version')['identity'] == version
    # A signed, headless mismatch executable exercises real subprocess identity
    # validation without relying on optimized Rust literal layout or another GUI.
    drift_app = base / 'drift.app'
    shutil.copytree(artifacts / 'Macscan.app', drift_app)
    drift_binary = drift_app / 'Contents/MacOS/macscan-desktop'
    drift_identity = dict(version, build_revision='different-clean-release')
    c_string = json.dumps(json.dumps({'identity': drift_identity}))
    source = '#include <stdio.h>\nint main(void) { puts(' + c_string + '); return 0; }\n'
    subprocess.run(['/usr/bin/clang', '-x', 'c', '-', '-o', str(drift_binary), '-Wl,-rpath,@executable_path/../Frameworks'], input=source, text=True, check=True, capture_output=True)
    subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(drift_binary)], check=True, capture_output=True)
    subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(drift_app)], check=True, capture_output=True)
    mismatch = install([str(installer), 'install', '--prefix', str(prefix), '--artifacts', str(artifacts), '--app', str(drift_app)], expected=3)
    assert 'version drift' in mismatch.stderr, mismatch.stderr
    assert tree(prefix) == before_install
    current_manifest = prefix / 'lib/macscan/current/manifest.json'
    manifest_bytes = current_manifest.read_bytes()
    changed = json.loads(manifest_bytes)
    changed['identity']['build_revision'] = 'edited-manifest'
    current_manifest.write_text(json.dumps(changed))
    install([str(installer), 'status', '--prefix', str(prefix)], expected=3)
    install(expected=3)
    assert current_manifest.exists()
    current_manifest.write_bytes(manifest_bytes)
    runtime = prefix / 'lib/macscan/current/libduckdb.dylib'
    hidden = runtime.with_name('hidden-library')
    runtime.rename(hidden)
    missing = subprocess.run(['macscan', '--version'], cwd=cwd, env=env, capture_output=True)
    assert missing.returncode == 3 and not missing.stdout
    assert json.loads(missing.stderr)['error']['operation'] == 'runtime_init'
    assert len(missing.stderr) <= 8192
    missing_ndjson = subprocess.run(['macscan', '--version', '--format', 'ndjson'], cwd=cwd, env=env, capture_output=True)
    assert missing_ndjson.returncode == 3 and json.loads(missing_ndjson.stderr)['type'] == 'error'
    raw = subprocess.run([str(prefix / 'lib/macscan/current/macscan'), '--version'], cwd=cwd, env=env, capture_output=True)
    assert raw.returncode != 0 and b'libduckdb' in raw.stderr
    hidden.rename(runtime)
    executable = prefix / 'lib/macscan/current/macscan'
    held = executable.with_name('held-executable')
    executable.rename(held)
    absent = subprocess.run(['macscan', '--version'], cwd=cwd, env=env, capture_output=True)
    assert absent.returncode == 3 and json.loads(absent.stderr)['error']['operation'] == 'runtime_init'
    held.rename(executable)
    unknown = prefix / 'lib/macscan/foreign'
    unknown.write_text('must survive')
    install(expected=3)
    install([str(installer), 'uninstall', '--prefix', str(prefix)], expected=3)
    assert unknown.read_text() == 'must survive'
    unknown.unlink()
    foreign_prefix = base / 'foreign prefix'
    (foreign_prefix / 'bin').mkdir(parents=True)
    foreign = foreign_prefix / 'bin/macscan'
    foreign.write_text('foreign command')
    install([str(installer), 'install', '--prefix', str(foreign_prefix), '--artifacts', str(artifacts), '--app', str(artifacts / 'Macscan.app')], expected=3)
    assert foreign.read_text() == 'foreign command'
    assert tree(store) == before_history
    install([str(installer), 'uninstall', '--prefix', str(prefix)])
    assert not (prefix / 'bin/macscan').exists() and not (prefix / 'lib/macscan').exists()
    assert tree(store) == before_history
    print('PASS installed release: discovery, roots/runs/members, analyses/growth, 3-run retention receipts, grouped fixture lookup, JSON/NDJSON bounds/progress, partial10/budget5/busy7/SIGINT130, wrong ID2/pruned ID4; copied runtime loading/missing runtime, atomic stage failure, drift, update/uninstall history preservation, foreign collisions. Owned prefix/store cleaned on exit.')
