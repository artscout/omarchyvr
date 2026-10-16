import sys;sys.path.insert(0,'.')
import usbp,collections
p=usbp.read(sys.argv[1])
devs=[int(a) for a in sys.argv[2].split(',')]
eps=[int(a,16) for a in sys.argv[3].split(',')]
g=collections.OrderedDict()
for x in p:
    if x['dev'] in devs and x['ep'] in eps and len(x['data'])>0 and x['tt']==1:
        key=(x['dev'],x['ep'],x['data'][:int(sys.argv[4])].hex())
        r=g.setdefault(key,[0,x['t'],x['t'],x['data']])
        r[0]+=1;r[2]=x['t']
for k,r in g.items():
    print(k[0],hex(k[1]),'n=%d %.3f..%.3f'%(r[0],r[1],r[2]),r[3].hex())
