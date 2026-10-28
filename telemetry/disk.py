"""The pipeline's disk (SP-09 design 3.4, 6 D3): every write the pipeline makes goes through one of these.

`RealDisk` is the file system. `SimDisk` models what a crash leaves on a POSIX file system: file contents survive
only up to their last `fsync`, plus a torn prefix of an append made after it; a directory's entries (a created,
replaced or renamed file) survive only up to the directory's last `fsync`; a created directory survives only once its
parent has been synced after its creation, and with it everything inside it. Every mutating operation is counted, and
`crash_at = k` raises `Crash` right after the k-th, so a run can be cut after every operation in turn."""
from __future__ import annotations

import os
from typing import Dict, List, Optional


class Crash(Exception):
    pass


class RealDisk:
    def __init__(self, record: Optional[str] = None):
        self.record = record                       # a file that receives every byte written (the egress scan's)

    def _log(self, data: bytes):
        if self.record:
            with open(self.record, "ab") as f:
                f.write(data + b"\n")

    def read(self, path: str) -> Optional[bytes]:
        try:
            with open(path, "rb") as f:
                return f.read()
        except FileNotFoundError:
            return None

    def write_new(self, path: str, data: bytes):
        self._log(data)
        with open(path, "wb") as f:
            f.write(data)

    def append(self, path: str, data: bytes):
        self._log(data)
        with open(path, "ab") as f:
            f.write(data)

    def fsync(self, path: str):
        fd = os.open(path, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)

    def rename(self, src: str, dst: str):
        os.replace(src, dst)

    def fsync_dir(self, d: str):
        self.fsync(d)

    def truncate(self, path: str, size: int):
        os.truncate(path, size)

    def remove(self, path: str):
        os.remove(path)

    def listdir(self, d: str) -> List[str]:
        return sorted(os.listdir(d)) if os.path.isdir(d) else []

    def is_dir(self, d: str) -> bool:
        return os.path.isdir(d)

    def makedirs(self, d: str):
        os.makedirs(d, exist_ok=True)


class _Inode:
    __slots__ = ("data", "synced")

    def __init__(self, data: bytes):
        self.data, self.synced = data, b""


class SimDisk:
    def __init__(self, crash_at: Optional[int] = None, torn: bool = True):
        self.dirs: Dict[str, Dict[str, Dict[str, _Inode]]] = {"/": {"cur": {}, "durable": {}}}   # dir -> {"cur", "durable"}
        self.dir_durable: Dict[str, bool] = {"/": True}
        self.written: List[bytes] = []            # every byte ever written, kept for the egress scan
        self.ops = 0
        self.crash_at, self.torn = crash_at, torn

    def _tick(self):
        self.ops += 1
        if self.crash_at is not None and self.ops == self.crash_at:
            raise Crash(f"crash after disk operation {self.ops}")

    def _dir(self, path: str):
        d, name = os.path.split(path)
        if d not in self.dirs:
            raise FileNotFoundError(d)
        return self.dirs[d], name

    def is_dir(self, d: str) -> bool:
        return d in self.dirs

    def makedirs(self, d: str):
        if d in self.dirs:
            return
        if os.path.dirname(d) not in self.dirs:
            raise FileNotFoundError(os.path.dirname(d))
        self.dirs[d] = {"cur": {}, "durable": {}}
        self.dir_durable[d] = False
        self._tick()

    def listdir(self, d: str) -> List[str]:
        return sorted(self.dirs.get(d, {"cur": {}})["cur"])

    def read(self, path: str) -> Optional[bytes]:
        dd, name = self._dir(path)
        ino = dd["cur"].get(name)
        return None if ino is None else ino.data

    def write_new(self, path: str, data: bytes):
        dd, name = self._dir(path)
        dd["cur"][name] = _Inode(data)
        self.written.append(data)
        self._tick()

    def append(self, path: str, data: bytes):
        dd, name = self._dir(path)
        if name not in dd["cur"]:
            dd["cur"][name] = _Inode(b"")
        dd["cur"][name].data += data
        self.written.append(data)
        self._tick()

    def fsync(self, path: str):
        dd, name = self._dir(path)
        ino = dd["cur"][name]
        ino.synced = ino.data
        self._tick()

    def rename(self, src: str, dst: str):
        sd, sn = self._dir(src)
        dd, dn = self._dir(dst)
        dd["cur"][dn] = sd["cur"].pop(sn)
        self._tick()

    def fsync_dir(self, d: str):
        dd = self.dirs[d]
        dd["durable"] = dict(dd["cur"])
        for c in self.dirs:
            if c != d and os.path.dirname(c) == d:
                self.dir_durable[c] = True
        self._tick()

    def truncate(self, path: str, size: int):
        dd, name = self._dir(path)
        ino = dd["cur"][name]
        ino.data = ino.data[:size]
        self._tick()

    def remove(self, path: str):
        dd, name = self._dir(path)
        del dd["cur"][name]
        self._tick()

    def crash(self):
        """What survives: each directory's durable entries, each file's synced contents plus, when the file was only
        appended to since, a torn prefix (half) of the unsynced tail."""
        def survives(x):
            return x == "/" or (self.dir_durable.get(x, False) and survives(os.path.dirname(x)))
        for x in [x for x in self.dirs if not survives(x)]:
            del self.dirs[x]
            del self.dir_durable[x]
        seen = set()
        for dd in self.dirs.values():
            dd["cur"] = dict(dd["durable"])
            for ino in dd["cur"].values():
                if id(ino) in seen:
                    continue
                seen.add(id(ino))
                tail = ino.data[len(ino.synced):] if ino.data.startswith(ino.synced) else b""
                ino.data = ino.synced + (tail[: len(tail) // 2] if self.torn else b"")
        self.crash_at = None
