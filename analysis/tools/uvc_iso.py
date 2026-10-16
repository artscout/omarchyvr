import struct
def read_iso(path,dev,ep):
    """yield (t, start_frame, [(off,len,status)], databuf) for IN completions of iso ep"""
    d=open(path,'rb').read(); off=24; t0=None
    while off+16<=len(d):
        s,us,cl,ol=struct.unpack('<IIII',d[off:off+16]); off+=16
        p=d[off:off+cl]; off+=cl
        t=s+us/1e6
        if t0 is None:t0=t
        hl,irp,st,fn,info,bus,dv,e,tt,dl=struct.unpack('<HQIHBHHBBI',p[:27])
        if tt==0 and dv==dev and e==ep and info&1:
            sf,n,err=struct.unpack('<III',p[27:39])
            ds=[struct.unpack('<III',p[39+12*i:51+12*i]) for i in range(n)]
            yield t-t0,sf,ds,p[hl:],dl
