#!/usr/bin/env python3
"""newc (cpio) initramfs üretir.

Kullanım: tools/mkinitramfs.py <cikti.cpio> <dizin>
Dış bağımlılık yok; kerneldeki fs/initramfs.rs bu biçimi açar.
"""

import os
import stat
import sys


def field(value):
    return b"%08x" % (value & 0xFFFFFFFF)


def header(ino, mode, nlink, size, name_size):
    return (
        b"070701"
        + field(ino)
        + field(mode)
        + field(0)          # uid
        + field(0)          # gid
        + field(nlink)
        + field(0)          # mtime
        + field(size)
        + field(0)          # devmajor
        + field(0)          # devminor
        + field(0)          # rdevmajor
        + field(0)          # rdevminor
        + field(name_size)
        + field(0)          # check
    )


def pad(data):
    return data + b"\0" * ((4 - len(data) % 4) % 4)


def entry(name, mode, data=b"", nlink=1, ino=1):
    name_bytes = name.encode() + b"\0"
    record = header(ino, mode, nlink, len(data), len(name_bytes))
    # Dolgu, kayıt başından itibaren 4 bayt hizasına göre hesaplanır.
    record += name_bytes
    record = pad(record)
    record += data
    record = pad(record)
    return record


def build(root, source):
    entries = []
    ino = [1]

    def next_ino():
        ino[0] += 1
        return ino[0]

    entries.append(entry(".", stat.S_IFDIR | 0o755, nlink=2, ino=next_ino()))

    for directory, subdirectories, files in os.walk(source):
        subdirectories.sort()
        files.sort()
        relative = os.path.relpath(directory, source)
        archive_directory = "." if relative == "." else "./" + relative

        if archive_directory != ".":
            entries.append(
                entry(archive_directory, stat.S_IFDIR | 0o755, nlink=2, ino=next_ino())
            )

        for name in files:
            path = os.path.join(directory, name)
            with open(path, "rb") as handle:
                data = handle.read()
            mode = stat.S_IFREG | (os.stat(path).st_mode & 0o777)
            archive_name = (archive_directory + "/" + name).replace("./", "./", 1)
            if archive_directory == ".":
                archive_name = "./" + name
            entries.append(entry(archive_name, mode, data, ino=next_ino()))

    entries.append(entry("TRAILER!!!", 0, ino=0))
    return b"".join(entries)


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        return 1

    output, source = sys.argv[1], sys.argv[2]
    archive = build(source, source)
    with open(output, "wb") as handle:
        handle.write(archive)
    print(f"initramfs: {output} ({len(archive)} bayt)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
