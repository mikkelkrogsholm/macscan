#!/usr/bin/env python3
"""Issue 14 clarification. Only creates/queries disposable, owned synthetic repos."""
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

GIT = '/usr/bin/git'
CAP = 65536
LOG = []
CONTROL_DEADLINE = None
CONTROL_CALLS = 0
CHECK_COUNT = 0


class Unverified(Exception):
    pass


def stamp():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def environment():
    # Deliberate allowlist: no inherited GIT_*, HOME, SSH or shell startup settings.
    return {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C', 'GIT_CONFIG_NOSYSTEM': '1',
            'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_TERMINAL_PROMPT': '0',
            'GIT_OPTIONAL_LOCKS': '0', 'GIT_NO_LAZY_FETCH': '1'}


def run(argv, cwd, stdin=b'', timeout=2.0, cap=CAP):
    """Drain both pipes, bounded storage/time; reap whole child group on failure."""
    global CONTROL_CALLS
    if CONTROL_DEADLINE is not None:
        CONTROL_CALLS += 1
        timeout = min(timeout, CONTROL_DEADLINE - time.monotonic())
        if timeout <= 0 or CONTROL_CALLS > 8:
            raise Unverified('overall control budget')
    entry = {'at': stamp(), 'argv': [str(x) for x in argv], 'cwd': str(cwd)}
    LOG.append(entry)
    started = time.monotonic()
    proc = subprocess.Popen(argv, cwd=cwd, env=environment(), stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            start_new_session=True)
    sel = selectors.DefaultSelector()
    output = [bytearray(), bytearray()]
    try:
        proc.stdin.write(stdin)
        proc.stdin.close()  # Input is restricted to one <= 1024-byte candidate.
        for i, stream in enumerate((proc.stdout, proc.stderr)):
            os.set_blocking(stream.fileno(), False)
            sel.register(stream, selectors.EVENT_READ, i)
        while sel.get_map():
            remaining = timeout - (time.monotonic() - started)
            if remaining <= 0:
                raise Unverified('subprocess timeout')
            for key, _ in sel.select(min(remaining, .05)):
                data = os.read(key.fileobj.fileno(), 8192)
                if not data:
                    sel.unregister(key.fileobj)
                else:
                    output[key.data].extend(data)
                    if len(output[key.data]) > cap:
                        raise Unverified('subprocess output budget')
        proc.wait(timeout=max(.001, timeout - (time.monotonic() - started)))
        entry.update(returncode=proc.returncode, stdout=bytes(output[0]).decode('utf-8', 'backslashreplace'),
                     stderr=bytes(output[1]).decode('utf-8', 'backslashreplace'))
        return proc.returncode, bytes(output[0]), bytes(output[1])
    except BaseException as exc:
        entry['failure'] = str(exc)
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()
        raise
    finally:
        sel.close()
        if not proc.stdin.closed:
            proc.stdin.close()
        proc.stdout.close()
        proc.stderr.close()
        entry['elapsed_seconds'] = round(time.monotonic() - started, 6)
        entry['reaped'] = proc.poll() is not None


OVERRIDES = ['--no-optional-locks', '--no-lazy-fetch', '--literal-pathspecs',
             '-c', 'core.fsmonitor=false', '-c', 'core.hooksPath=/dev/null',
             '-c', 'core.excludesFile=/dev/null', '-c', 'core.untrackedCache=false',
             '-c', 'maintenance.auto=false', '-c', 'gc.auto=0']


def owned(path, parent):
    path = Path(os.path.abspath(path))
    if not path.is_relative_to(parent):
        raise Unverified('Git pointer outside owned fixture parent')
    # Reject symlink components before opening any pointed-to state.
    current = parent
    for part in path.relative_to(parent).parts:
        current /= part
        if current.is_symlink():
            raise Unverified('symlink in Git data path')
    return path


def small_read(path, parent):
    owned(path, parent)
    try:
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > CAP:
            raise Unverified('invalid/oversize Git control file')
        with path.open('rb') as stream:
            data = stream.read(CAP + 1)
        if len(data) > CAP:
            raise Unverified('Git control file budget')
        return data
    except OSError as exc:
        raise Unverified(f'Git control read: {exc.__class__.__name__}') from exc


def preflight(repo, parent, scratch):
    marker = repo / '.git'
    owned(marker, parent)
    if marker.is_dir():
        gitdir = marker
    else:
        raw = small_read(marker, parent)
        if not raw.startswith(b'gitdir: ') or raw.count(b'\n') > 1:
            raise Unverified('malformed .git marker')
        try:
            pointer = raw[8:].strip().decode('utf-8')
        except UnicodeError as exc:
            raise Unverified('malformed .git marker encoding') from exc
        if not pointer or '\x00' in pointer:
            raise Unverified('malformed .git pointer')
        gitdir = owned(repo / pointer, parent)
    common = gitdir
    if (gitdir / 'commondir').exists():
        raw = small_read(gitdir / 'commondir', parent)
        try:
            pointer = raw.strip().decode('utf-8')
        except UnicodeError as exc:
            raise Unverified('malformed commondir') from exc
        if not pointer or '\n' in pointer or '\x00' in pointer:
            raise Unverified('malformed commondir')
        common = owned(gitdir / pointer, parent)
    allowed = {'core.repositoryformatversion', 'core.filemode', 'core.bare',
               'core.logallrefupdates', 'core.ignorecase', 'core.precomposeunicode',
               'extensions.worktreeconfig', 'core.sparsecheckout',
               'core.sparsecheckoutcone', 'index.sparse'}
    worktree_config = False
    resolved_config = {}
    for config in (common / 'config', gitdir / 'config.worktree'):
        if config.name == 'config.worktree' and not config.exists():
            continue
        small_read(config, parent)
        rc, data, _ = run([GIT, 'config', '--no-includes', '--file', str(config),
                           '--null', '--list'], scratch)
        if rc:
            raise Unverified('malformed/unreadable config')
        seen = set()
        for item in data.split(b'\0'):
            if not item:
                continue
            key, sep, value = item.partition(b'\n')
            key = key.decode('ascii', 'strict').lower()
            if key not in allowed or key in seen or not sep:
                raise Unverified('include/unknown/ambiguous config: ' + key)
            seen.add(key)
            if key in resolved_config and resolved_config[key] != value:
                raise Unverified('ambiguous common/worktree config')
            resolved_config[key] = value
            if key != 'core.repositoryformatversion' and value not in (b'true', b'false'):
                raise Unverified('ambiguous boolean config')
            if key == 'core.bare' and value != b'false':
                raise Unverified('bare or ambiguous config')
            if key == 'core.repositoryformatversion' and value not in (b'0', b'1'):
                raise Unverified('unknown repository format')
            if key in ('core.sparsecheckout', 'core.sparsecheckoutcone', 'index.sparse') and value != b'true':
                raise Unverified('ambiguous sparse config')
            if key == 'extensions.worktreeconfig':
                if value != b'true':
                    raise Unverified('ambiguous worktree config')
                worktree_config = True
        if config.name == 'config.worktree' and not worktree_config:
            raise Unverified('unexpected worktree config')
    for data_dir in (gitdir, common):
        exclude = data_dir / 'info/exclude'
        if exclude.exists() or exclude.is_symlink():
            small_read(exclude, parent)
    small_read(gitdir / 'HEAD', parent)
    small_read(gitdir / 'index', parent)  # Missing index is never "untracked".
    return gitdir, common


def current_check(repo, candidate, parent, scratch, injected_denial=False):
    global CONTROL_DEADLINE, CONTROL_CALLS, CHECK_COUNT
    CHECK_COUNT += 1
    CONTROL_DEADLINE, CONTROL_CALLS = time.monotonic() + 5, 0
    result = {'checked_at': stamp(), 'candidate': candidate, 'state': 'unverified',
              'safety': 'review; no regenerability or deletion claim'}
    started = time.monotonic()
    try:
        if CHECK_COUNT > 32:
            raise Unverified('candidate count budget')
        if (len(candidate.encode()) > 1024 or len(candidate.split('/')) > 8 or candidate.startswith('/') or
                any(x in ('', '.', '..') for x in candidate.split('/')) or '\x00' in candidate):
            raise Unverified('candidate boundary')
        gitdir, common = preflight(repo, parent, scratch)
        if injected_denial:
            raise PermissionError('injected denial at Git index read')
        parts = candidate.split('/')
        # Only path metadata and ignore rules; no ordinary file-content access.
        for depth in range(len(parts) + 1):
            ancestor = owned(repo.joinpath(*parts[:depth]), parent)
            nested_marker = ancestor / '.git'
            if depth and (nested_marker.exists() or nested_marker.is_symlink()):
                raise Unverified('nested repository marker; review')
            ignore = ancestor / '.gitignore'
            if ignore.is_symlink():
                raise Unverified('symlink ignore rules')
            if ignore.exists():
                small_read(ignore, parent)
        args = [GIT, *OVERRIDES, '--git-dir=' + str(gitdir), '--work-tree=' + str(repo)]
        rc, data, _ = run([*args, 'ls-files', '--cached', '--stage', '--sparse', '-z', '--', candidate], repo)
        if rc:
            raise Unverified('index query failed')
        # Exact candidate/descendants first; whole bounded index protects gitlink,
        # sparse and case-folded ancestors that exact pathspec can omit.
        rc, all_data, _ = run([*args, 'ls-files', '--cached', '--stage', '--sparse', '-z'], repo)
        if rc:
            raise Unverified('ancestor index query failed')
        tracked = False
        for row in (data + all_data).split(b'\0'):
            if not row:
                continue
            match = re.fullmatch(rb'([0-7]{6}) ([0-9a-f]{40,64}) ([0-3])\t(.+)', row, re.S)
            if not match:
                raise Unverified('malformed index evidence')
            mode, _, stage, raw_path = match.groups()
            path = raw_path.decode('utf-8', 'strict').rstrip('/')
            folded, target = path.casefold(), candidate.casefold()
            related = (folded == target or folded.startswith(target + '/') or target.startswith(folded + '/'))
            if related and (stage != b'0' or mode in (b'160000', b'040000')):
                raise Unverified('gitlink/sparse/conflict ancestor or descendant; review')
            if folded == target or folded.startswith(target + '/'):
                tracked = True
        if tracked:
            result['state'] = 'tracked; review'
        else:
            ignore_args = [a for a in args if a != '--literal-pathspecs']
            rc, data, _ = run([*ignore_args, 'check-ignore', '--stdin', '-z', '--verbose'], repo,
                             (candidate + '\0').encode())
            if rc not in (0, 1):
                raise Unverified('ignore query error')
            fields = data.split(b'\0')
            if data:
                if len(fields) != 5 or fields[-1] or fields[3] != candidate.encode():
                    raise Unverified('malformed ignore evidence')
                source, line, pattern, _ = fields[:4]
                if not source or not line.isdigit() or not pattern:
                    raise Unverified('incomplete ignore evidence')
                result['rule'] = {'source': source.decode('utf-8', 'backslashreplace'),
                                  'line': int(line), 'pattern': pattern.decode('utf-8', 'backslashreplace')}
                result['state'] = ('not locally ignored; review' if pattern.startswith(b'!')
                                   else 'locally ignored; review')
            elif rc == 1:
                result['state'] = 'not locally ignored; review'
            else:
                raise Unverified('missing ignore evidence')
        if time.monotonic() - started > 5:
            raise Unverified('overall control budget')
    except (Unverified, OSError, UnicodeError, subprocess.SubprocessError) as exc:
        result['state'] = 'unverified'
        result['reason'] = str(exc)
    finally:
        CONTROL_DEADLINE = None
    result['elapsed_seconds'] = round(time.monotonic() - started, 6)
    return result


def git_state(parent):
    """Only hash files in owned Git data directories, never ordinary contents."""
    records = []
    count = 0
    for root in sorted(parent.glob('*/.git')):
        if not root.is_dir() or root.is_symlink():
            continue
        for directory, dirs, files in os.walk(root, followlinks=False):
            for name in sorted(dirs + files):
                path = Path(directory) / name
                count += 1
                if count > 1024:
                    raise Unverified('Git state file-count cap')
                info = path.lstat()
                if stat.S_ISLNK(info.st_mode):
                    records.append((str(path.relative_to(parent)), 'symlink'))
                elif stat.S_ISREG(info.st_mode):
                    if info.st_size > 2 * 1024 * 1024:
                        raise Unverified('Git state size cap')
                    try:
                        digest = hashlib.sha256(path.read_bytes()).hexdigest()
                    except PermissionError:
                        digest = 'permission-denied; contents unobserved'
                    records.append((str(path.relative_to(parent)), digest,
                                    info.st_size, info.st_mtime_ns, info.st_mode))
    return records


def experiment(parent):
    scratch = parent / 'nonrepo'
    scratch.mkdir()
    rc, version, _ = run([GIT, '--version'], scratch)
    match = re.search(rb'git version (\d+)\.(\d+)', version)
    if rc or not match or tuple(map(int, match.groups())) < (2, 36):
        raise Unverified('requires system Git >= 2.36')
    fixture_commands = []

    def mutate(repo, *args, stdin=b''):
        rc, _, err = run([GIT, *OVERRIDES, '-c', 'user.name=Synthetic Probe',
                          '-c', 'user.email=probe@example.invalid', '-C', str(repo), *args], scratch, stdin)
        fixture_commands.append(len(LOG) - 1)
        if rc:
            raise Unverified('fixture setup failed: ' + err.decode(errors='replace'))

    def create(name):
        repo = parent / name
        repo.mkdir()
        mutate(repo, 'init', '--quiet')
        return repo

    def write(repo, relative, text):
        dest = repo / relative
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(text)

    repo = create('normal')
    write(repo, 'target/tracked.txt', 'synthetic tracked build-name data\n')
    write(repo, 'target/valuable.db', 'VALUABLE SYNTHETIC LOCAL DATA: NEVER SAFE TO DELETE\n')
    write(repo, '.gitignore', 'target/\ncache/*\n!cache/keep.txt\nignored\n')
    write(repo, 'cache/keep.txt', 'synthetic keep\n')
    write(repo, 'ignoredness/file', 'prefix collision\n')
    write(repo, 'literal[1].txt', 'literal\n')
    mutate(repo, 'add', '-f', '--', 'target/tracked.txt', '.gitignore', 'literal[1].txt')
    mutate(repo, 'commit', '--quiet', '-m', 'owned synthetic baseline')
    hook_marker = parent / 'hook-executed'
    hooks = repo / '.git/hooks'
    write(repo, '.git/hooks/post-index-change', '#!/bin/sh\ntouch "' + str(hook_marker) + '"\n')
    (hooks / 'post-index-change').chmod(0o700)
    linked = parent / 'linked'
    mutate(repo, 'worktree', 'add', '--quiet', '-b', 'owned-linked', str(linked))
    # Installed hook is disabled even for fixture commands above.
    nested = create('nested-parent')
    (nested / 'child').mkdir()
    mutate(nested / 'child', 'init', '--quiet')
    write(nested, 'seed', 'seed\n')
    mutate(nested, 'add', 'seed')
    mutate(nested, 'commit', '--quiet', '-m', 'seed')
    sub = create('gitlink')
    write(sub, 'seed', 'seed\n')
    mutate(sub, 'add', 'seed')
    mutate(sub, 'commit', '--quiet', '-m', 'seed')
    rc, oid, _ = run([GIT, '--git-dir=' + str(sub / '.git'), 'rev-parse', 'HEAD'], scratch)
    assert rc == 0
    mutate(sub, 'update-index', '--add', '--cacheinfo', '160000,' + oid.decode().strip() + ',module')
    sparse = create('sparse')
    write(sparse, 'keep/a', 'synthetic keep\n')
    write(sparse, 'hidden/a', 'synthetic sparse\n')
    mutate(sparse, 'add', '--', 'keep', 'hidden')
    mutate(sparse, 'commit', '--quiet', '-m', 'synthetic sparse baseline')
    mutate(sparse, 'sparse-checkout', 'init', '--cone', '--sparse-index')
    mutate(sparse, 'sparse-checkout', 'set', 'keep')
    conflict = parent / 'conflict'
    shutil.copytree(repo, conflict)
    rc, blob, _ = run([GIT, '--git-dir=' + str(conflict / '.git'),
                       'rev-parse', 'HEAD:target/tracked.txt'], scratch)
    assert rc == 0
    mutate(conflict, 'update-index', '--force-remove', '--', 'target/tracked.txt')
    mutate(conflict, 'update-index', '--index-info',
           stdin=b'100644 ' + blob.strip() + b' 1\ttarget/tracked.txt\n')
    # All following fixtures are derived from our own synthetic repository only.
    variants = {}
    for name in ('missing-index', 'corrupt-index', 'bad-marker', 'bad-config',
                 'external-include', 'unknown-extension', 'symlink-ignore', 'unreadable', 'outside-pointer',
                 'broken-nested-marker', 'oversize-ignore', 'missing-marker', 'malformed-commondir'):
        variants[name] = parent / name
        shutil.copytree(repo, variants[name])
    (variants['missing-index'] / '.git/index').unlink()
    (variants['corrupt-index'] / '.git/index').write_bytes(b'NOT AN INDEX\0')
    shutil.rmtree(variants['bad-marker'] / '.git')
    (variants['bad-marker'] / '.git').write_text('invalid marker\n')
    (variants['bad-config'] / '.git/config').write_text('[broken\n')
    with (variants['external-include'] / '.git/config').open('a') as f:
        f.write('[include]\n path = /never-read-owned-probe-outside\n')
    with (variants['unknown-extension'] / '.git/config').open('a') as f:
        f.write('[extensions]\n unknownProbe = true\n')
    (variants['symlink-ignore'] / '.gitignore').unlink()
    (variants['symlink-ignore'] / '.gitignore').symlink_to(parent / 'never-read-ignore')
    shutil.rmtree(variants['outside-pointer'] / '.git')
    (variants['outside-pointer'] / '.git').write_text('gitdir: /never-read-owned-probe-outside\n')
    (variants['broken-nested-marker'] / 'child').mkdir()
    (variants['broken-nested-marker'] / 'child/.git').symlink_to(parent / 'absent-owned-git')
    (variants['oversize-ignore'] / '.gitignore').write_bytes(b'#' * (CAP + 1))
    shutil.rmtree(variants['missing-marker'] / '.git')
    (variants['malformed-commondir'] / '.git/commondir').write_text('bad\nextra\n')
    protected_index = variants['unreadable'] / '.git/index'
    protected_index.chmod(0)
    snapshot = {'captured_at': stamp(), 'synthetic_metadata': {'target/valuable.db': {'logical_bytes': 50}},
                'git_evidence': None}
    frozen_snapshot = json.dumps(snapshot, sort_keys=True)
    before = git_state(parent)
    control_start = len(LOG)
    results = {}

    def check(name, location, candidate, expected):
        result = current_check(location, candidate, parent, scratch)
        results[name] = result
        assert result['state'] == expected, (name, result, expected)

    try:
        check('tracked-build-name', repo, 'target', 'tracked; review')
        check('valuable-ignored', repo, 'target/valuable.db', 'locally ignored; review')
        check('negated-rule', repo, 'cache/keep.txt', 'not locally ignored; review')
        check('prefix-collision', repo, 'ignoredness', 'not locally ignored; review')
        check('literal-path', repo, 'literal[1].txt', 'tracked; review')
        check('linked-worktree', linked, 'target/tracked.txt', 'tracked; review')
        check('nested-repository', nested, 'child/data', 'unverified')
        check('submodule-ancestor', sub, 'module/data', 'unverified')
        check('sparse-ancestor', sparse, 'hidden/data', 'unverified')
        assert 'sparse/conflict' in results['sparse-ancestor']['reason']
        check('conflicted-index', conflict, 'target', 'unverified')
        assert 'sparse/conflict' in results['conflicted-index']['reason']
        check('literal-untracked', repo, 'literal*.txt', 'not locally ignored; review')
        check('casefold-protection', repo, 'TARGET', 'tracked; review')
        for name in variants:
            if name != 'unreadable':
                candidate = 'child/data' if name == 'broken-nested-marker' else 'target/valuable.db'
                check(name, variants[name], candidate, 'unverified')
        actual = current_check(variants['unreadable'], 'target/valuable.db', parent, scratch)
        results['permission-denial-native'] = actual
        if actual['state'] != 'unverified':
            results['permission-denial-injected'] = current_check(repo, 'target/valuable.db', parent, scratch, True)
            assert results['permission-denial-injected']['state'] == 'unverified'
        control_end = len(LOG)
        after = git_state(parent)
        if before != after:
            LOG.append({'git_state_difference': {'before': [x for x in before if x not in after],
                                                'after': [x for x in after if x not in before]}})
            raise AssertionError('control phase modified Git data')
        assert not hook_marker.exists(), 'hook executed'
        assert json.dumps(snapshot, sort_keys=True) == frozen_snapshot
    finally:
        protected_index.chmod(0o644)
    # Fixture-only index change BETWEEN current checks, outside control phase.
    first = current_check(repo, 'target/valuable.db', parent, scratch)
    mutate(repo, 'add', '-f', '--', 'target/valuable.db')
    second = current_check(repo, 'target/valuable.db', parent, scratch)
    assert first['state'] == 'locally ignored; review' and second['state'] == 'tracked; review'
    assert first['checked_at'] < second['checked_at']
    assert json.dumps(snapshot, sort_keys=True) == frozen_snapshot
    budget_tests = {}
    for name, code, timeout, cap in (
            ('timeout', 'import time; time.sleep(2)', .05, CAP),
            ('output-budget', 'import os; os.write(1,b"x"*100000)', 2, 1024)):
        try:
            run([sys.executable, '-c', code], scratch, timeout=timeout, cap=cap)
            raise AssertionError(name + ' did not stop')
        except Unverified as exc:
            budget_tests[name] = str(exc)
        assert LOG[-1]['reaped']
    return {'git_version': version.decode().strip(), 'environment': environment(),
            'results': results, 'snapshot': snapshot, 'snapshot_unchanged': True, 'current_candidate_checks': CHECK_COUNT,
            'index_change_between_checks': [first, second], 'git_state_unchanged_in_control': True,
            'hook_executed': False, 'budget_tests': budget_tests,
            'fixture_mutation_command_indices': fixture_commands,
            'control_command_range_half_open': [control_start, control_end],
            'commands': LOG}


def main():
    if sys.argv[1:] != ['--owned-local-fixtures']:
        print('No work performed. Opt in with --owned-local-fixtures (no path arguments supported).')
        return 0 if not sys.argv[1:] else 2
    parent = Path(tempfile.mkdtemp(prefix='macscan-git-evidence-')).resolve()
    report = {'fixture_parent': str(parent), 'started_at': stamp()}
    try:
        report.update(experiment(parent))
    except BaseException as exc:
        report.update(failure=str(exc), commands=LOG)
        raise
    finally:
        shutil.rmtree(parent)  # Unique parent created by this process only.
        report['fixtures_removed'] = not parent.exists()
        report['finished_at'] = stamp()
        print(json.dumps(report, indent=2))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
