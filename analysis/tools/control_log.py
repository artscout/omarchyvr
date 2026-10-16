import sys,struct;sys.path.insert(0,'.')
import usbp
p=usbp.read(sys.argv[1]); devs=[int(a) for a in sys.argv[2].split(',')]
tn={0:'GET_STATUS',1:'CLEAR_FEATURE',3:'SET_FEATURE',5:'SET_ADDRESS',6:'GET_DESCRIPTOR',7:'SET_DESCRIPTOR',8:'GET_CONFIG',9:'SET_CONFIG',10:'GET_INTERFACE',11:'SET_INTERFACE'}
hid={1:'GET_REPORT',2:'GET_IDLE',3:'GET_PROTOCOL',9:'SET_REPORT',0xa:'SET_IDLE',0xb:'SET_PROTOCOL'}
uvc={1:'SET_CUR',0x81:'GET_CUR',0x82:'GET_MIN',0x83:'GET_MAX',0x84:'GET_RES',0x85:'GET_LEN',0x86:'GET_INFO',0x87:'GET_DEF'}
pend={}
for i,x in enumerate(p):
    if x['dev'] not in devs or x['tt']!=2 or x['ep']&0x7f!=0: continue
    if x['stage']==0 and x['info']==0 and len(x['data'])==8:
        pend[x['irp']]=x; 
    elif x['stage']==2 and x['info']==0:  # data out stage
        s=pend.get(x['irp'])
        if s: s['out']=x['data']
    elif x['stage']==2 and x['info']==1 or (x['stage']==3):
        s=pend.pop(x['irp'],None)
        if s is None: continue
        d=s['data'];bm,br,wv,wi,wl=struct.unpack('<BBHHH',d)
        typ=(bm>>5)&3
        nm={0:tn,1:uvc if (bm&0x1f)==1 and (wi>>8)!=0 or True else hid,2:hid}.get(typ,{}).get(br,hex(br))
        if typ==1 and bm&0x1f==1 and (wi&0xff) in (0,1) and dev_cam(s) if False else False: pass
        data=s.get('out') if not bm&0x80 else x['data']
        print('%d %.4f dev%d bm=%02x %s(%02x) wV=%04x wI=%04x wL=%d %s %s'%(s['n'],s['t'],s['dev'],bm,nm,br,wv,wi,wl,'IN' if bm&0x80 else 'OUT',(data or b'').hex()))
