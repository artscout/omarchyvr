import sys;sys.path.insert(0,'.')
import usbp
p=usbp.read(sys.argv[1]); dev=int(sys.argv[2]); ep=int(sys.argv[3],16)
buf=b'';t0=None
for x in p:
    if x['dev']==dev and x['ep']==ep and x['tt']==1 and len(x['data'])>0:
        if t0 is None:t0=x['t']
        buf+=x['data'].rstrip(b'\0')
print('first t',t0)
print(buf.decode('latin1'))
