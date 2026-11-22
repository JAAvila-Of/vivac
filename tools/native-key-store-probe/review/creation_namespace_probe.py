"""Retain explicitly selected entries in a disposable custody namespace."""
import hashlib
import os
import stat

import creation_draft_probe as draft

PREFIX = ".vivac-openssh-preparing-"
MAX_ENTRIES = 16
require = draft.require


def package_name(value):
    return (type(value) is str and (draft.fixed.installation(value)
            or (value.startswith(PREFIX) and draft.fixed.hexadecimal(value[len(PREFIX):], 32))))


def admission_name(value):
    return type(value) is str and draft.fixed.installation(value)


def validated_names(packages=(), admissions=()):
    result = []
    for values, accepts in ((packages, package_name), (admissions, admission_name)):
        require(type(values) in (tuple, list) and len(values) <= MAX_ENTRIES,
                "preserved namespace selection exceeded its bound")
        require(all(accepts(value) for value in values) and len(set(values)) == len(values),
                "preserved namespace names differ")
        result.append(tuple(values))
    return tuple(result)


def select_stage(members, preserved_names=()):
    preserved, _ = validated_names(preserved_names)
    require(type(members) in (tuple, list) and all(type(name) is str for name in members)
            and len(members) == len(preserved) + 1 and len(set(members)) == len(members)
            and set(preserved).issubset(members), "draft preserved membership differs")
    candidates = [name for name in members if name not in preserved]
    require(len(candidates) == 1 and candidates[0].startswith(PREFIX), "draft stage membership differs")
    return candidates[0]


def entry_summary(name, metadata, members):
    return {"name": name, "metadata": list(metadata), "members": [
        {"name": member, "metadata": list(values[2]), "sha256": hashlib.sha256(values[1]).hexdigest()}
        for member, values in sorted(members.items())]}


def _summary_metadata(value, directory, limit=None):
    require(type(value) is list and len(value) == 9 and all(type(item) is int for item in value),
            "preserved metadata fields differ")
    require(value[0] >= 0 and value[1] > 0 and value[3:5] == [65534, 65534]
            and stat.S_IMODE(value[2]) == (0o700 if directory else 0o600)
            and (stat.S_ISDIR(value[2]) if directory else stat.S_ISREG(value[2]))
            and value[5] == (2 if directory else 1), "preserved metadata profile differs")
    if not directory:
        require(0 < value[6] <= limit, "preserved member size differs")


def validate_summary(value):
    require(type(value) is dict and set(value) == {"packages", "admissions"},
            "preserved namespace summary fields differ")
    for kind in ("packages", "admissions"):
        entries = value[kind]
        require(type(entries) is list and len(entries) <= MAX_ENTRIES, "preserved summary count differs")
        names = []
        for entry in entries:
            require(type(entry) is dict and set(entry) == {"name", "metadata", "members"},
                    "preserved entry summary fields differ")
            require((package_name if kind == "packages" else admission_name)(entry["name"]),
                    "preserved entry name differs")
            names.append(entry["name"])
            _summary_metadata(entry["metadata"], True)
            expected = {"key.openssh": 65536, "manifest.json": 2048} if kind == "packages" else {"record.json": 2048}
            members = entry["members"]
            require(type(members) is list and len(members) == len(expected), "preserved member summary count differs")
            selected = []
            for member in members:
                require(type(member) is dict and set(member) == {"name", "metadata", "sha256"}
                        and member["name"] in expected, "preserved member summary fields differ")
                selected.append(member["name"])
                _summary_metadata(member["metadata"], False, expected[member["name"]])
                require(member["metadata"][0] == entry["metadata"][0]
                        and draft.fixed.hexadecimal(member["sha256"], 64), "preserved member provenance differs")
            require(selected == sorted(expected), "preserved member summary membership differs")
        require(names == sorted(set(names)), "preserved summary names differ")
    return value


def _directory(fd, device=None):
    info = os.fstat(fd)
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == info.st_gid == 65534
            and stat.S_IMODE(info.st_mode) == 0o700
            and (device is None or info.st_dev == device) and not os.listxattr(fd),
            "preserved directory profile differs")
    return draft.metadata(info)


def _file(fd, device, limit):
    info = os.fstat(fd)
    require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 65534
            and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1
            and info.st_dev == device and 0 < info.st_size <= limit and not os.listxattr(fd),
            "preserved file profile differs")
    return draft.metadata(info)


def _read(fd, before, limit):
    value = os.pread(fd, limit + 1, 0)
    require(len(value) == before[6] and draft.metadata(os.fstat(fd)) == before,
            "preserved file changed during reading")
    return value


class PreservedNamespace:
    def __init__(self, packages=(), admissions=()):
        selected = validated_names(packages, admissions)
        self._roots = {}
        self._entries = {"packages": {}, "admissions": {}}
        self._closed = False
        try:
            for kind, path in (("packages", draft.PACKAGES), ("admissions", draft.ADMISSIONS)):
                descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
                try:
                    captured = _directory(descriptor)
                except BaseException:
                    os.close(descriptor)
                    raise
                self._roots[kind] = (descriptor, captured, path)
            self.validate()
            self.extend(packages=selected[0], admissions=selected[1])
        except BaseException:
            self.close()
            raise

    @property
    def package_names(self):
        return tuple(self._entries["packages"])

    @property
    def admission_names(self):
        return tuple(self._entries["admissions"])

    def _capture(self, kind, name):
        root, parent, _ = self._roots[kind]
        directory = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=root)
        members = {}
        try:
            before = _directory(directory, parent[0])
            require(before[5] == 2, "preserved entry link count differs")
            expected = {"key.openssh": 65536, "manifest.json": 2048} if kind == "packages" else {"record.json": 2048}
            require(set(os.listdir(directory)) == set(expected), "preserved entry membership differs")
            for member, limit in expected.items():
                descriptor = os.open(member, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=directory)
                members[member] = (descriptor, None, None)
                info = _file(descriptor, parent[0], limit)
                raw = _read(descriptor, info, limit)
                require(draft.metadata(os.stat(member, dir_fd=directory, follow_symlinks=False)) == info,
                        "preserved named member changed")
                members[member] = (descriptor, raw, info)
            entry = (directory, before, members)
            self._validate_entry(kind, name, entry)
            return entry
        except BaseException:
            os.close(directory)
            for descriptor, _, _ in members.values():
                os.close(descriptor)
            raise

    def _validate_entry(self, kind, name, entry):
        root, parent, _ = self._roots[kind]
        directory, before, members = entry
        fresh = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=root)
        try:
            require(_directory(directory, parent[0]) == before == _directory(fresh, parent[0]),
                    "preserved directory identity changed")
            require(set(os.listdir(directory)) == set(members) == set(os.listdir(fresh)),
                    "preserved entry members changed")
            for member, (descriptor, raw, info) in members.items():
                limit = 65536 if member == "key.openssh" else 2048
                require(_file(descriptor, parent[0], limit) == info and _read(descriptor, info, limit) == raw,
                        "preserved held member changed")
                current = os.open(member, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=fresh)
                try:
                    require(_file(current, parent[0], limit) == info and _read(current, info, limit) == raw,
                            "preserved named member changed")
                finally:
                    os.close(current)
                require(draft.metadata(os.stat(member, dir_fd=directory, follow_symlinks=False)) == info,
                        "preserved held directory member changed")
            require(_directory(directory, parent[0]) == before == _directory(fresh, parent[0])
                    == draft.metadata(os.stat(name, dir_fd=root, follow_symlinks=False)),
                    "preserved directory changed during validation")
        finally:
            os.close(fresh)

    def validate(self):
        require(not self._closed and set(self._roots) == {"packages", "admissions"},
                "preserved namespace is not retained")
        for kind, (descriptor, before, path) in self._roots.items():
            # Parent entry counts, size and timestamps may change for other credentials.
            current = _directory(descriptor)
            require(current[:5] == before[:5] == draft.metadata(path.lstat())[:5],
                    "preserved namespace root identity changed")
            for name, entry in self._entries[kind].items():
                self._validate_entry(kind, name, entry)
        return self

    def extend(self, packages=(), admissions=()):
        selected = validated_names(packages, admissions)
        self.validate()
        for kind, values in zip(("packages", "admissions"), selected):
            require(len(self._entries[kind]) + len(values) <= MAX_ENTRIES
                    and not set(values).intersection(self._entries[kind]),
                    "preserved extension duplicates or exceeds selected entries")
        for kind, values in zip(("packages", "admissions"), selected):
            for name in values:
                self._entries[kind][name] = self._capture(kind, name)
        self.validate()
        return self

    def summarize(self):
        self.validate()
        value = {kind: [entry_summary(name, entry[1], entry[2]) for name, entry in sorted(entries.items())]
                 for kind, entries in self._entries.items()}
        return validate_summary(value)

    def close(self):
        if self._closed:
            return
        self._closed = True
        for entries in self._entries.values():
            for directory, _, members in entries.values():
                for descriptor, _, _ in members.values():
                    os.close(descriptor)
                os.close(directory)
        for descriptor, _, _ in self._roots.values():
            os.close(descriptor)
        self._entries = {"packages": {}, "admissions": {}}
        self._roots = {}
