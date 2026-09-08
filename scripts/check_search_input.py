#!/usr/bin/env python3
"""Real keyboard regression check. Requires an X11 display and libXtst.

Run: python3 scripts/check_search_input.py [path/to/picasa-rs]
Opens its own window and temporary library; never uses the normal photo library.
The check takes keyboard focus; run on an idle desktop or an isolated X11 display.
"""
import ctypes as C
import os
import re
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time


def main():
    x = C.CDLL("libX11.so.6")
    xt = C.CDLL("libXtst.so.6")
    ptr, win = C.c_void_p, C.c_ulong
    signatures = {
        "XOpenDisplay": ([C.c_char_p], ptr),
        "XDefaultRootWindow": ([ptr], win),
        "XQueryTree": ([ptr, win, C.POINTER(win), C.POINTER(win), C.POINTER(C.POINTER(win)), C.POINTER(C.c_uint)], C.c_int),
        "XFetchName": ([ptr, win, C.POINTER(C.c_char_p)], C.c_int),
        "XFree": ([ptr], C.c_int),
        "XSetInputFocus": ([ptr, win, C.c_int, C.c_ulong], C.c_int),
        "XRaiseWindow": ([ptr, win], C.c_int),
        "XResizeWindow": ([ptr, win, C.c_uint, C.c_uint], C.c_int),
        "XStringToKeysym": ([C.c_char_p], C.c_ulong),
        "XKeysymToKeycode": ([ptr, C.c_ulong], C.c_uint),
        "XFlush": ([ptr], C.c_int),
        "XCloseDisplay": ([ptr], C.c_int),
    }
    for name, (args, result) in signatures.items():
        getattr(x, name).argtypes = args
        getattr(x, name).restype = result
    xt.XTestFakeKeyEvent.argtypes = [ptr, C.c_uint, C.c_int, C.c_ulong]
    display = x.XOpenDisplay(None)
    assert display, "An X11 display is required"
    root = x.XDefaultRootWindow(display)

    def windows(parent=root):
        children = C.POINTER(win)()
        n, r, p = C.c_uint(), win(), win()
        if not x.XQueryTree(display, parent, C.byref(r), C.byref(p), C.byref(children), C.byref(n)):
            return set()
        result = set(children[i] for i in range(n.value))
        x.XFree(children)
        for child in list(result):
            result.update(windows(child))
        return result

    def key(name, pressed):
        code = x.XKeysymToKeycode(display, x.XStringToKeysym(name.encode()))
        assert code, name
        xt.XTestFakeKeyEvent(display, code, pressed, 0)
        x.XFlush(display)

    def type_keys(text):
        for char in text:
            name = "space" if char == " " else char
            key(name, True)
            key(name, False)
            time.sleep(.04)

    def control(name):
        key("Control_L", True)
        type_keys(name)
        key("Control_L", False)
        time.sleep(.1)

    initial = windows()
    with tempfile.TemporaryDirectory(prefix="pic-search-ui-") as fixture:
        data = Path(fixture) / "data" / "picasa-rs"
        data.mkdir(parents=True)
        with sqlite3.connect(data / "library.db") as db:
            db.execute("CREATE TABLE folders (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, name TEXT, parent_id INTEGER, imported_root BOOLEAN DEFAULT 1, watched BOOLEAN DEFAULT 0)")
            db.executemany("INSERT INTO folders(path,name) VALUES (?,?)", [("/fixture/Drone", "Drone"), ("/fixture/Elves", "Elves")])
            db.executemany("INSERT INTO folders(path,name) VALUES (?,?)", [(f"/fixture/Drone{i:02}", f"Drone{i:02}") for i in range(12)])
        env = dict(os.environ, GDK_BACKEND="x11", PICASA_TRACE="1", GTK_A11Y="none", GIO_USE_VFS="local", XDG_DATA_HOME=str(data.parent), XDG_CACHE_HOME=fixture + "/cache")
        binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/picasa-rs"
        with tempfile.TemporaryFile(mode="w+") as log:
            app = subprocess.Popen(["dbus-run-session", "--", binary], env=env, stdout=log, stderr=log)
            try:
                target = None
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline and target is None:
                    for candidate in windows() - initial:
                        title = C.c_char_p()
                        x.XFetchName(display, candidate, C.byref(title))
                        name = title.value or b""
                        if title.value:
                            x.XFree(title)
                        if name.startswith(b"PIC -"):
                            target = candidate
                            break
                    time.sleep(.1)
                assert target, "Test window did not appear"
                x.XRaiseWindow(display, target)
                x.XSetInputFocus(display, target, 1, 0)
                x.XFlush(display)
                time.sleep(2)  # Let initial mapping and compositor placement settle.
                for width in (1440, 900, 740, 1440):
                    x.XResizeWindow(display, target, width, 850)
                    x.XFlush(display)
                    time.sleep(.6)
                    x.XSetInputFocus(display, target, 1, 0)
                    x.XFlush(display)
                    time.sleep(.2)
                    for query in ("drone", "elves", "drone trip"):
                        control("f")
                        control("a")
                        key("BackSpace", True)
                        key("BackSpace", False)
                        time.sleep(.4)
                        log.seek(0, 2)
                        start = log.tell()
                        type_keys(query[:2])
                        time.sleep(.7)  # Popup has time to map and take input.
                        type_keys(query[2:])
                        time.sleep(.7)
                        log.seek(start)
                        output = log.read()
                        assert f'query="{query}"' in output, f"Typing stopped at width {width}:\n{output}"
                        print(f"PASS width={width} query={query}", flush=True)
                    control("a")
                    type_keys("dr")
                    time.sleep(.7)
                    log.seek(0, 2)
                    start = log.tell()
                    for _ in range(11):
                        key("Down", True)
                        key("Down", False)
                        time.sleep(.04)
                    key("Up", True)
                    key("Up", False)
                    time.sleep(.2)
                    log.seek(start)
                    output = log.read()
                    assert "suggestion_selected index=10" in output, output
                    positions = re.findall(r"suggestion_selected index=9 scroll=([0-9.]+)", output)
                    assert positions and float(positions[-1]) > 0, output
                    key("Return", True)
                    key("Return", False)
                    time.sleep(.5)
                    log.seek(start)
                    output = log.read()
                    # Name order: Drone, Drone00, ..., Drone08 (index 9, ID 11).
                    assert "suggestion_activated folder_id=11" in output, output
                    print(f"PASS width={width} Up/Down scroll and Enter opens selected folder", flush=True)
                    control("f")
                    type_keys("el")
                    time.sleep(.6)
                    key("Down", True)
                    key("Down", False)
                    # Typing after navigating must still go into the entry.
                    type_keys("ves")
                    time.sleep(.6)
                    key("Escape", True)
                    key("Escape", False)
                    log.seek(0, 2)
                    start = log.tell()
                    type_keys("more")
                    time.sleep(.6)
                    log.seek(start)
                    output = log.read()
                    assert 'query="elvesmore"' in output, output
                    print(f"PASS width={width} typing after navigation and Escape preserves text", flush=True)
                    if width == 1440:
                        control("f")
                        key("Shift_L", True)
                        key("Tab", True)
                        key("Tab", False)
                        key("Shift_L", False)
                        time.sleep(.2)
                        log.seek(0, 2)
                        start = log.tell()
                        type_keys("dr")
                        time.sleep(.7)
                        type_keys("one")
                        time.sleep(.7)
                        log.seek(start)
                        output = log.read()
                        assert "type_to_search started chars=1" in output, output
                        assert 'query="drone"' in output, output
                        print("PASS typing outside search starts a fresh query with the first character", flush=True)
            except Exception:
                log.seek(0)
                print(log.read()[-10000:], file=sys.stderr)
                raise
            finally:
                app.terminate()
                app.wait(timeout=5)
    x.XCloseDisplay(display)


if __name__ == "__main__":
    main()
