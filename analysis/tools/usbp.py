import struct,sys
def read(path):
    f=open(path,'rb'); d=f.read()
    magic=d[:4]
    le = magic in (b'\xd4\xc3\xb2\xa1',b'\x4d\x3c\xb2\xa1')
    nano = magic in (b'\x4d\x3c\xb2\xa1',b'\xa1\xb2\x3c\x4d')
    e='<' if le else '>'
    off=24; t0=None
    if magic==b'\x0a\x0d\x0d\x0a': raise SystemExit('pcapng')
    out=[]
    n=0
    while off+16<=len(d):
        s,us,cl,ol=struct.unpack(e+'IIII',d[off:off+16]); off+=16
        p=d[off:off+cl]; off+=cl; n+=1
        t=s+us/(1e9 if nano else 1e6)
        if t0 is None:t0=t
        hl,irp,st,fn,info,bus,dev,ep,tt,dl=struct.unpack('<HQIHBHHBBI',p[:27])
        stage=p[27] if tt==2 and hl>=28 else None
        data=p[hl:]
        out.append(dict(n=n,t=t-t0,abs=t,st=st,fn=fn,info=info,dev=dev,ep=ep,tt=tt,dl=dl,stage=stage,data=data,irp=irp))
    return out
