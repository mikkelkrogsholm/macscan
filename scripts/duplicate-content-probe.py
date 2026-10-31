#!/usr/bin/env python3
"""Issue 15: bounded DATA-content experiment on disposable owned fixtures only."""
import ctypes
import errno
import hashlib
import json
import os
import signal
import stat
import sys
import tempfile
import time

ALGORITHM = 'sha256-prefix-suffix-4096/full-sha256/v1'
CHUNK = 64 * 1024
MAX_FILES = 32
MAX_READ = 8 * 1024 * 1024
STOP = False


class Unverified(Exception):
    pass


def identity(s):
    return (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)


def event(**fields):
    print(json.dumps(fields, sort_keys=True), file=sys.stderr, flush=True)


class Limits:
    def __init__(self):
        self.read = 0
        self.fixture_read = 0
        self.deadline = time.monotonic() + 20

    def check(self):
        if STOP:
            raise Unverified('Stop requested')
        if time.monotonic() >= self.deadline:
            raise Unverified('deadline')
        if self.read >= MAX_READ:
            raise Unverified('invocation content-read budget')


class Owned:
    """No enumeration; only registered, single-component names are ever used."""
    def __init__(self, limits):
        self.limits = limits
        self.path = None
        self.fd = None
        self.names = set()

    def __enter__(self):
        self.limits.check()
        # Fixed OS temporary area; never accept a caller-supplied root.
        self.path = tempfile.mkdtemp(prefix='macscan-duplicate-owned-',
                                     dir='/private/tmp' if sys.platform == 'darwin' else '/tmp')
        try:
            self.fd = os.open(self.path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            s = os.fstat(self.fd)
            self.root_id = (s.st_dev, s.st_ino)
            self.verify_root()
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def verify_root(self):
        a = os.fstat(self.fd)
        b = os.stat(self.path, follow_symlinks=False)
        if (not stat.S_ISDIR(a.st_mode) or not stat.S_ISDIR(b.st_mode)
                or (a.st_dev, a.st_ino) != self.root_id
                or (b.st_dev, b.st_ino) != self.root_id):
            raise Unverified('root replaced')

    def register(self, name):
        if not name or name in ('.', '..') or '/' in name or '\x00' in name:
            raise Unverified('not a single-component name')
        if name not in self.names and len(self.names) >= MAX_FILES:
            raise Unverified('candidate memory/file cap')
        self.names.add(name)

    def create(self, name, data):
        self.limits.check()
        self.register(name)
        self.verify_root()
        fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                     0o600, dir_fd=self.fd)
        try:
            view = memoryview(data)
            while view:
                self.limits.check()
                view = view[os.write(fd, view):]
        finally:
            os.close(fd)

    def metadata(self, name):
        if name not in self.names:
            raise Unverified('unowned name')
        self.limits.check()
        self.verify_root()
        s = os.stat(name, dir_fd=self.fd, follow_symlinks=False)
        if not stat.S_ISREG(s.st_mode):
            raise Unverified('nonregular/symlink rejected before content open')
        return s

    def remove_files(self):
        # Cleanup deliberately ignores Stop/read budget, and never walks a tree.
        self.verify_root()
        for name in sorted(self.names):
            try:
                os.unlink(name, dir_fd=self.fd)
            except FileNotFoundError:
                pass

    def __exit__(self, *_):
        try:
            if self.fd is not None:
                self.remove_files()
                # Never remove a different root after failed identity validation.
                self.verify_root()
                os.rmdir(self.path)
            elif self.path is not None:
                # Only reached if the initial directory open failed.
                # Leave cleanup unknown rather than remove an unverified path.
                raise Unverified('root never opened; owned cleanup unknown')
        finally:
            if self.fd is not None:
                os.close(self.fd)
                self.fd = None


class Probe:
    def __init__(self, owned, byte_cap=MAX_READ, hook=None, stop=None):
        self.owned = owned
        self.byte_cap = byte_cap
        self.read = 0
        self.hook = hook
        self.stop = stop or (lambda: False)
        self.stage = 'metadata'
        self.opened = []

    def check(self):
        self.owned.limits.check()
        if self.stop():
            raise Unverified('Stop requested (injected)')
        if self.read >= self.byte_cap:
            raise Unverified('content-read budget')

    def stable(self, name, expected, fd=None):
        self.owned.verify_root()
        path = self.owned.metadata(name)
        if identity(path) != identity(expected):
            raise Unverified('path identity/size/times changed: ' + name)
        if fd is not None:
            s = os.fstat(fd)
            if not stat.S_ISREG(s.st_mode) or identity(s) != identity(expected):
                raise Unverified('descriptor identity/size/times changed: ' + name)

    def digest(self, name, expected, full):
        self.check()
        self.stable(name, expected)
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                     dir_fd=self.owned.fd)
        try:
            self.stable(name, expected, fd)
            self.opened.append(name)
            h = hashlib.sha256()
            size = expected.st_size
            spans = [(0, size)] if full else [(0, min(size, 4096))]
            if not full and size > 4096:
                spans.append((max(4096, size - 4096), min(4096, size - 4096)))
            for offset, remaining in spans:
                os.lseek(fd, offset, os.SEEK_SET)
                while remaining:
                    self.check()
                    request = min(CHUNK, remaining, self.byte_cap - self.read,
                                  MAX_READ - self.owned.limits.read)
                    data = os.read(fd, request)
                    self.read += len(data)
                    self.owned.limits.read += len(data)
                    event(stage=self.stage, file=name, actual_content_bytes_read=self.read)
                    if self.hook:
                        self.hook(self, name, fd)
                    if not data:
                        raise Unverified('short read: ' + name)
                    h.update(data)
                    remaining -= len(data)
            self.stable(name, expected, fd)
            event(stage=self.stage, file=name, actual_content_bytes_read=self.read)
            return h.digest()
        finally:
            os.close(fd)

    def run(self, names, residency='owned-local-by-construction'):
        result = {'algorithm': ALGORITHM, 'status': 'unverified', 'groups': [],
                  'logical_duplicate_content_bytes': None, 'hardlink_aliases': [],
                  'classification': residency}
        try:
            self.check()
            # Injected classification, not provider detection or live evidence.
            if residency != 'owned-local-by-construction':
                raise Unverified('SIMULATED provider/dataless/unknown residency rejected')
            if len(names) > MAX_FILES or len(set(names)) != len(names):
                raise Unverified('candidate memory/file cap or repeated name')
            metadata = {n: self.owned.metadata(n) for n in names}
            sizes = {}
            for name, s in metadata.items():
                sizes.setdefault(s.st_size, []).append(name)
            candidates = []
            for members in sizes.values():
                if len(members) < 2:
                    continue
                representatives = {}
                for name in members:
                    s = metadata[name]
                    key = (s.st_dev, s.st_ino)
                    if key in representatives:
                        result['hardlink_aliases'].append([representatives[key], name])
                    else:
                        representatives[key] = name
                if len(representatives) > 1:
                    candidates.append(list(representatives.values()))
            self.stage = 'partial-filter'
            collisions = []
            for members in candidates:
                partial = {}
                for name in members:
                    self.check()
                    digest = self.digest(name, metadata[name], False)
                    partial.setdefault(digest, []).append(name)
                collisions.extend(v for v in partial.values() if len(v) > 1)
            self.stage = 'full-sha256'
            groups = []
            for members in collisions:
                hashes = {}
                for name in members:
                    self.check()
                    digest = self.digest(name, metadata[name], True)
                    hashes.setdefault(digest, []).append(name)
                for digest, matches in hashes.items():
                    if len(matches) > 1:
                        groups.append({'files': matches, 'size_bytes': metadata[matches[0]].st_size,
                                       'sha256': digest.hex(), 'verification': 'full-sha256-DATA'})
            self.stage = 'final-revalidation'
            # Recheck even skipped metadata and aliases; no partially verified group.
            for name, s in metadata.items():
                self.check()
                self.stable(name, s)
            result.update(status='verified-DATA-observation', groups=groups,
                          logical_duplicate_content_bytes=sum(
                              (len(g['files']) - 1) * g['size_bytes'] for g in groups))
        except (Unverified, OSError) as exc:
            result.update(reason=str(exc), groups=[], logical_duplicate_content_bytes=None,
                          hardlink_aliases=[])
        result.update(stage=self.stage, actual_content_bytes_read=self.read,
                      content_opened=self.opened)
        return result


def native_clone(o, source, target):
    o.limits.check()
    o.metadata(source)
    if sys.platform != 'darwin':
        return {'status': 'unknown', 'reason': 'native Darwin clone unavailable'}
    o.register(target)
    o.verify_root()
    libc = ctypes.CDLL(None, use_errno=True)
    clone = getattr(libc, 'clonefileat', None)
    if clone is None:
        return {'status': 'unknown', 'reason': 'native clonefileat symbol unavailable'}
    clone.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    clone.restype = ctypes.c_int
    if clone(o.fd, source.encode(), o.fd, target.encode(), 0):
        return {'status': 'unknown', 'errno': ctypes.get_errno()}
    return {'status': 'native-clonefileat-succeeded', 'source': source, 'target': target,
            'relation': 'native clone creation; separate from DATA verification'}


def resource_fork(o, name):
    if sys.platform != 'darwin':
        return {'status': 'unknown', 'reason': 'Darwin resource fork unavailable'}
    o.limits.check()
    expected = o.metadata(name)
    verifier = Probe(o)
    verifier.stable(name, expected)
    fd = os.open(name, os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=o.fd)
    try:
        verifier.stable(name, expected, fd)
        value = b'independent synthetic resource fork'
        libc = ctypes.CDLL(None, use_errno=True)
        setter = libc.fsetxattr
        setter.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p,
                           ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
        setter.restype = ctypes.c_int
        getter = libc.fgetxattr
        getter.argtypes = setter.argtypes
        getter.restype = ctypes.c_ssize_t
        attr = b'com.apple.ResourceFork'
        payload = ctypes.create_string_buffer(value)
        if setter(fd, attr, payload, len(value), 0, 0):
            raise OSError(ctypes.get_errno(), 'fsetxattr resource fork')
        expected = o.metadata(name)
        verifier.stable(name, expected, fd)
        o.limits.check()
        output = ctypes.create_string_buffer(len(value))
        read = getter(fd, attr, output, min(len(value), MAX_READ - o.limits.read), 0, 0)
        if read > 0:
            o.limits.read += read
            o.limits.fixture_read += read
        verifier.stable(name, expected, fd)
        if read != len(value) or output.raw != value:
            raise OSError(ctypes.get_errno(), 'fgetxattr resource fork mismatch')
        source_metadata = o.metadata('a')
        other = os.open('a', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=o.fd)
        try:
            verifier.stable('a', source_metadata, other)
            o.limits.check()
            absent = getter(other, attr, None, 0, 0, 0)
            absent_errno = ctypes.get_errno()
            verifier.stable('a', source_metadata, other)
            if absent != -1 or absent_errno != errno.ENOATTR:
                raise OSError(absent_errno, 'expected absent source resource fork')
        finally:
            os.close(other)
        return {'status': 'native-resource-fork-written-and-read', 'file': name,
                'source_resource_fork': 'absent (ENOATTR)', 'bytes': len(value)}
    except (OSError, AttributeError) as exc:
        return {'status': 'unknown', 'reason': str(exc), 'errno': getattr(exc, 'errno', None)}
    finally:
        os.close(fd)


def exercise(o):
    outcomes = []

    def check(label, condition, result):
        if not condition:
            raise AssertionError(label + ': ' + json.dumps(result))
        outcomes.append({'case': label, 'result': result})

    a = b'A' * (3 * CHUNK)
    # Same prefix/suffix and size, different middle: must reach full hash.
    collision = a[:CHUNK] + b'B' * CHUNK + a[2 * CHUNK:]
    o.create('a', a)
    o.create('b', a)
    o.create('collision', collision)
    o.create('different', b'D' * len(a))
    o.create('empty-a', b'')
    o.create('empty-b', b'')
    o.create('unique-size', b'unique')
    o.register('alias')
    o.limits.check()
    os.link('a', 'alias', src_dir_fd=o.fd, dst_dir_fd=o.fd, follow_symlinks=False)
    clone = native_clone(o, 'a', 'clone')
    fork = resource_fork(o, 'b')
    names = ['a', 'b', 'collision', 'different', 'empty-a', 'empty-b', 'unique-size', 'alias']
    if clone['status'] == 'native-clonefileat-succeeded':
        names.append('clone')
    p = Probe(o)
    r = p.run(names)
    expected_equal = {'a', 'b'} | ({'clone'} if 'clone' in names else set())
    check('identical-independent-DATA/full-hash',
          r['status'] == 'verified-DATA-observation'
          and any(set(g['files']) == expected_equal for g in r['groups']), r)
    check('same-size-different-and-partial-collision-rejected',
          all('collision' not in g['files'] and 'different' not in g['files'] for g in r['groups'])
          and p.opened.count('collision') == 2 and p.opened.count('different') == 1,
          {'collision_content_opens': p.opened.count('collision'),
           'different_content_opens': p.opened.count('different')})
    check('hardlink-alias-separate', r['hardlink_aliases'] == [['a', 'alias']], r['hardlink_aliases'])
    check('zero-length-and-exact-logical-counter',
          any(g['size_bytes'] == 0 and len(g['files']) == 2 for g in r['groups'])
          and r['logical_duplicate_content_bytes'] == (len(expected_equal) - 1) * len(a),
          {'logical_duplicate_content_bytes': r['logical_duplicate_content_bytes']})
    check('unique-size-skipped-before-content-open', 'unique-size' not in p.opened, p.opened)
    outcomes.extend([{'case': 'native-clone-separate-relation', 'result': clone},
                     {'case': 'resource-fork-DATA-only-equality', 'result': fork}])
    o.remove_files()

    o.create('target', b'T' * (2 * CHUNK))
    o.register('symlink')
    o.limits.check()
    os.symlink('target', 'symlink', dir_fd=o.fd)
    r = Probe(o).run(['target', 'symlink'])
    check('symlink-before-content-read', r['status'] == 'unverified'
          and r['actual_content_bytes_read'] == 0 and not r['content_opened'], r)
    o.remove_files()

    for mode in ('mutation', 'replacement', 'Stop', 'budget', 'IO-error', 'deadline'):
        o.create('a', a)
        o.create('b', a)
        fired = False

        def hook(probe, name, fd):
            nonlocal fired
            if fired or probe.stage != 'full-sha256':
                return
            fired = True
            if mode == 'mutation':
                writer = os.open(name, os.O_WRONLY | os.O_NOFOLLOW, dir_fd=o.fd)
                try:
                    os.pwrite(writer, b'Z', CHUNK)
                finally:
                    os.close(writer)
            elif mode == 'replacement':
                os.unlink(name, dir_fd=o.fd)
                o.create(name, a)
            elif mode == 'IO-error':
                raise OSError(errno.EIO, 'injected owned-fixture read failure')

        cap = 2 * 8192 + CHUNK + 17 if mode == 'budget' else MAX_READ
        p = Probe(o, byte_cap=cap, hook=hook,
                  stop=lambda: mode == 'Stop' and fired)
        old_deadline = o.limits.deadline
        if mode == 'deadline':
            o.limits.deadline = time.monotonic() - 1
        try:
            r = p.run(['a', 'b'])
        finally:
            o.limits.deadline = old_deadline
        expected_reason = {
            'mutation': 'path identity/size/times changed',
            'replacement': 'path identity/size/times changed',
            'Stop': 'Stop requested', 'budget': 'content-read budget',
            'IO-error': 'injected owned-fixture read failure', 'deadline': 'deadline',
        }[mode]
        check(mode + '-no-verified-partial-group', r['status'] == 'unverified'
              and expected_reason in r['reason']
              and r['groups'] == [] and r['logical_duplicate_content_bytes'] is None, r)
        if mode == 'budget':
            check('precise-partial-read-accounting', r['actual_content_bytes_read'] == cap, r)
        o.remove_files()

    for classification in ('SIMULATED-known-provider', 'SIMULATED-dataless', 'SIMULATED-unknown-residency'):
        r = Probe(o).run(['a'], residency=classification)
        check(classification, r['status'] == 'unverified' and r['actual_content_bytes_read'] == 0
              and not r['content_opened'], r)
    return outcomes


def main():
    # Parse before temporary creation, metadata, or content operations.
    if sys.argv[1:] not in (['--owned-local-fixtures'], ['--owned-local-fixtures', '--self-test']):
        print('No work performed. Use --owned-local-fixtures [--self-test]; no paths accepted.')
        return 0 if not sys.argv[1:] else 2
    if not hasattr(os, 'O_NOFOLLOW'):
        print(json.dumps({'status': 'unverified', 'reason': 'O_NOFOLLOW unavailable'}))
        return 1
    def stopped(*_):
        global STOP
        STOP = True
    signal.signal(signal.SIGINT, stopped)
    signal.signal(signal.SIGTERM, stopped)
    limits = Limits()
    try:
        with Owned(limits) as owned:
            fixture = owned.path
            outcomes = exercise(owned)
        print(json.dumps({'scope': 'owned synthetic fixtures only; no product activation',
                          'algorithm': ALGORITHM, 'outcomes': outcomes,
                          'actual_content_bytes_read': limits.read,
                          'hash_content_bytes_read': limits.read - limits.fixture_read,
                          'fixture_resource_fork_bytes_read': limits.fixture_read,
                          'content_read_cap_bytes': MAX_READ, 'max_files': MAX_FILES,
                          'chunk_bytes': CHUNK, 'deadline_seconds': 20, 'cleanup': 'completed',
                          'removed_fixture': fixture}, indent=2, sort_keys=True))
        return 0
    except (OSError, Unverified, AssertionError) as exc:
        print(json.dumps({'status': 'unverified', 'reason': str(exc),
                          'actual_content_bytes_read': limits.read}), file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
