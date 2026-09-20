#!/usr/bin/env python3
"""End-to-end standalone helper test: list directory, stat exact JPEG, read JPEG bytes.
Usage: python3 helper/smoke.py 10.0.0.1 '/mnt/4TBP/Other/Tat Sing/20190917_184453.jpg'
No PIC process, GVfs, mount(2), sudo, or cached image involved.
"""
import os
import struct
import subprocess
import sys

HELPER = os.environ.get("PIC_NFS_HELPER_PATH", "/usr/local/libexec/pic-nfs-helper")


def read_exact(stream, size):
    data = stream.read(size)
    if len(data) != size:
        raise RuntimeError(f"helper ended while reading {size} bytes (got {len(data)})")
    return data


def u32(stream):
    return struct.unpack(">I", read_exact(stream, 4))[0]


def u64(stream):
    return struct.unpack(">Q", read_exact(stream, 8))[0]


def request(proc, op, host, path):
    host, path = host.encode(), path.encode()
    frame = op.encode() + struct.pack(">I", len(host)) + host
    frame += struct.pack(">I", len(path)) + path
    proc.stdin.write(frame)
    proc.stdin.flush()
    status = read_exact(proc.stdout, 1)
    if status == b"E":
        raise RuntimeError(read_exact(proc.stdout, u32(proc.stdout)).decode(errors="replace"))
    if status != b"O":
        raise RuntimeError(f"unexpected helper status: {status!r}")


def main():
    if len(sys.argv) != 3 or not sys.argv[2].startswith("/"):
        sys.exit("usage: python3 helper/smoke.py HOST '/absolute/export/path/photo.jpg'")
    host, path = sys.argv[1:]
    directory = os.path.dirname(path)
    proc = subprocess.Popen([HELPER], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        request(proc, "L", host, directory)
        files = []
        while True:
            size = u32(proc.stdout)
            if size == 0:
                break
            if size == 0xFFFFFFFF:
                raise RuntimeError(read_exact(proc.stdout, u32(proc.stdout)).decode(errors="replace"))
            if size > 4096:
                raise RuntimeError("invalid listing")
            name = read_exact(proc.stdout, size).decode(errors="replace")
            directory_flag = read_exact(proc.stdout, 1)[0]
            _file_size = u64(proc.stdout)
            _mtime = u64(proc.stdout)
            files.append((name, directory_flag))
        print(f"LIST OK: {len(files)} entries")
        if not any(name == os.path.basename(path) for name, _ in files):
            raise RuntimeError("exact JPEG not present in directory listing")
        request(proc, "S", host, path)
        is_directory = read_exact(proc.stdout, 1)[0]
        file_size = u64(proc.stdout)
        _mtime = u64(proc.stdout)
        if is_directory or not file_size:
            raise RuntimeError("JPEG is not a nonempty regular file")
        print(f"STAT OK: {file_size} bytes")
        request(proc, "R", host, path)
        first = b""
        total = 0
        while True:
            size = u32(proc.stdout)
            if size == 0:
                break
            if size == 0xFFFFFFFF:
                raise RuntimeError(read_exact(proc.stdout, u32(proc.stdout)).decode(errors="replace"))
            if size > 65536:
                raise RuntimeError("invalid read chunk")
            data = read_exact(proc.stdout, size)
            if len(first) < 3:
                first += data[:3 - len(first)]
            total += len(data)
            if total > 512 * 1024 * 1024:
                raise RuntimeError("read limit exceeded")
        if first != b"\xff\xd8\xff":
            raise RuntimeError(f"wrong JPEG signature: {first.hex()}")
        if total != file_size:
            raise RuntimeError(f"incomplete JPEG: read {total}, stat said {file_size}")
        print(f"READ OK: {total} bytes; JPEG SIGNATURE OK: {first.hex(' ')}")
        print("PRIVATE HELPER LIVE TEST PASSED")
    finally:
        proc.stdin.close()
        proc.wait(timeout=5)


if __name__ == "__main__":
    main()
