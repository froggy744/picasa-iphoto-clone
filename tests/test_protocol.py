#!/usr/bin/env python3
"""Build local libnfs mock, verify portable stdio binary protocol and safety."""
import os
from pathlib import Path
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory() as d:
    helper = Path(d) / 'pic-nfs-helper'
    subprocess.run(['cc','-std=c11','-D_DEFAULT_SOURCE','-Wall','-Wextra','-Werror',
                    '-I'+str(ROOT/'tests/mock'),
                    str(ROOT/'helper/pic-nfs-helper-portable.c'),
                    str(ROOT/'tests/mock_libnfs.c'),'-o',str(helper)],check=True)
    with subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE) as p:
        def take(n):
            data = p.stdout.read(n)
            assert len(data) == n, ('short response', data, n)
            return data
        def u32(): return struct.unpack('>I',take(4))[0]
        def u64(): return struct.unpack('>Q',take(8))[0]
        def request(op,path,host='mock.local'):
            host,path=host.encode(),path.encode()
            p.stdin.write(op + struct.pack('>I',len(host))+host+
                          struct.pack('>I',len(path))+path)
            p.stdin.flush()
            status=take(1)
            if status==b'E': return ('error',take(u32()).decode())
            assert status==b'O',status
            return ('ok',None)
        assert request(b'S','/export')==('ok',None)
        assert take(1)==b'\1' and u64()==0 and u64()==0
        assert request(b'S','/export/img.jpg')==('ok',None)
        assert take(1)==b'\0' and u64()==14 and u64()==42
        assert request(b'L','/export')==('ok',None)
        name=take(u32());assert name==b'img.jpg'
        assert take(1)==b'\0' and u64()==14 and u64()==42 and u32()==0
        assert request(b'R','/export/img.jpg')==('ok',None)
        payload=take(u32());assert payload.startswith(b'\xff\xd8\xff') and len(payload)==14
        assert u32()==0
        for bad in ('/export/../secret','/export/./img.jpg','/export//img.jpg','/export\\secret'):
            assert request(b'S',bad)[0]=='error'
        assert request(b'S','/export','wrong.local')[0]=='error'
        p.stdin.close()
        assert p.wait(timeout=3)==0
print('PASS: mock C helper compiled, stat/list/read/framing/traversal/error paths')
