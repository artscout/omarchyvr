import sys,struct
sys.path.insert(0,'C:/Users/User/pimax-re/analysis/tools')
from uvc_iso import read_iso
PCAP='C:/Users/User/pimax-re/captures2/20261013-130650-full-packets-hub2.pcap'
def chunks():
    """returns list of frames: dict(idx, hdrs[(pkt_in_frame,hdrbytes,t)], chunks[(offset,bytes)], size)"""
    frames=[];cur=dict(chunks=[],hdrs=[],off=0,npk=0,t=None)
    for k,(t,sf,ds,b,dl) in enumerate(read_iso(PCAP,29,0x81)):
        for i,(o,l,s) in enumerate(ds):
            if l==0: continue
            if o+l<=len(b) or o+12<=len(b):
                h=b[o:o+12]; cur['hdrs'].append((cur['npk'],h,t,sf,i,k))
                pay=b[o+12:o+l]
                if len(pay)>0 and l>12: cur['chunks'].append((cur['off'],pay))
            cur['off']+=max(l-12,0); cur['npk']+=1
            if l==12:
                cur['size']=cur['off']; frames.append(cur); cur=dict(chunks=[],hdrs=[],off=0,npk=0,t=None)
    return frames
