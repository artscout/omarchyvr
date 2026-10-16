#!/usr/bin/env python3
"""Controller motion metrics from a PIMAX_CTRL_IMU_LOG log (pimax-hands fuse.rs) or a `pimax-ctrltrack fusereplay`
output: jitter at rest (gyro still), fix-to-fix steps by LED count, shown-pose steps per state transition, orientation
and position error at re-entry after an out-of-view gap. Usage: ctrl_motion.py LOG.csv [...]"""
import sys, math, bisect, statistics as st
def qmul(a,b):
    return [a[3]*b[0]+a[0]*b[3]+a[1]*b[2]-a[2]*b[1], a[3]*b[1]-a[0]*b[2]+a[1]*b[3]+a[2]*b[0], a[3]*b[2]+a[0]*b[1]-a[1]*b[0]+a[2]*b[3], a[3]*b[3]-a[0]*b[0]-a[1]*b[1]-a[2]*b[2]]
def conj(q): return [-q[0],-q[1],-q[2],q[3]]
def ang(a,b):
    d=qmul(conj(a),b); s=math.sqrt(d[0]**2+d[1]**2+d[2]**2); return math.degrees(2*math.atan2(s,abs(d[3])))
def rot(q,v):
    p=qmul(qmul(q,v+[0]),conj(q)); return p[:3]
def pct(v,p):
    v=sorted(v); return v[min(len(v)-1,int(len(v)*p))] if v else float('nan')
def load(path):
    I=[];O=[];P=[]
    for l in open(path):
        f=l.strip().split(',')
        if f[0]=='I' and len(f)>=6: I.append((int(f[1]),max(abs(int(f[3])),abs(int(f[4])),abs(int(f[5])))))
        elif f[0]=='O':
            O.append(dict(t=int(f[1]),p=list(map(float,f[2:5])),q=list(map(float,f[5:9])),hp=list(map(float,f[9:12])),hq=list(map(float,f[12:16])),rms=float(f[16]),leds=int(f[17]),v=f[18] if len(f)>18 else 'ok'))
        elif f[0]=='P':
            P.append(dict(t=int(f[1]),s=int(f[2]),p=list(map(float,f[3:6])),q=list(map(float,f[6:10])),hp=list(map(float,f[10:13])),hq=list(map(float,f[13:17]))))
    return I,O,P
def headrel(p,hp,hq): return rot(conj(hq),[p[i]-hp[i] for i in range(3)])
def rest_windows(I, thr=25, min_s=0.6):
    w=[];start=None;last=None
    for t,g in I:
        if g<=thr:
            if start is None: start=t
            last=t
        else:
            if start is not None and (last-start)/1e9>=min_s: w.append((start,last))
            start=None
    if start is not None and (last-start)/1e9>=min_s: w.append((start,last))
    return w
def analyse(path, label):
    I,O,P=load(path)
    ok=[o for o in O if o['v']=='ok']
    W=rest_windows(I)
    # jitter at rest: optical fixes and shown poses, deviation from the window mean (head frame and world)
    def jit(items, key_p, key_q, frame):
        dp=[];dq=[]
        for a,b in W:
            sel=[x for x in items if a<=x['t']<=b]
            if len(sel)<5: continue
            ps=[headrel(x['p'],x['hp'],x['hq']) if frame=='head' else x['p'] for x in sel]
            m=[sum(p[i] for p in ps)/len(ps) for i in range(3)]
            dp+= [1000*math.dist(p,m) for p in ps]
            # orientation: vs the median-ish first sample after removing head rotation
            qs=[qmul(conj(x['hq']),x['q']) if frame=='head' else x['q'] for x in sel]
            dq+= [ang(qs[k],qs[k-1]) for k in range(1,len(qs))]
        return dp,dq
    print(f"== {label}: {len(I)} gyro samples, {len(O)} fixes ({len(ok)} ok), {len(P)} shown samples, rest windows {len(W)} ({sum((b-a)/1e9 for a,b in W):.0f} s)")
    for name,items in [('optical fixes',ok),('shown poses',[p for p in P if p['s']!=0])]:
        for fr in ['world','head']:
            dp,dq=jit(items,'p','q',fr)
            print(f"  jitter at rest, {name}, {fr} frame: position |dev| p50 {pct(dp,.5):.1f} p90 {pct(dp,.9):.1f} mm; frame-to-frame rotation p50 {pct(dq,.5):.2f} p90 {pct(dq,.9):.2f} deg (n={len(dp)})")
    # handovers: consecutive ok fixes 40 ms apart at rest, step size by LED-count change
    same=[];chg=[]
    for a,b in zip(ok,ok[1:]):
        if b['t']-a['t']>45e6: continue
        if not any(x<=a['t']<=y for x,y in W): continue
        s=1000*math.dist(headrel(a['p'],a['hp'],a['hq']),headrel(b['p'],b['hp'],b['hq']))
        (chg if abs(a['leds']-b['leds'])>=3 else same).append(s)
    print(f"  at rest, fix-to-fix step: LED count same(+-2) p50 {pct(same,.5):.1f} p90 {pct(same,.9):.1f} mm (n={len(same)}); LED count changes by 3+ p50 {pct(chg,.5):.1f} p90 {pct(chg,.9):.1f} mm (n={len(chg)})")
    byled={}
    for a,b in zip(ok,ok[1:]):
        if b['t']-a['t']>45e6 or not any(x<=a['t']<=y for x,y in W): continue
        k=min(a['leds'],b['leds']); k='5-6' if k<=6 else '7-9' if k<=9 else '10+'
        byled.setdefault(k,[]).append(1000*math.dist(headrel(a['p'],a['hp'],a['hq']),headrel(b['p'],b['hp'],b['hq'])))
    print("  at rest, fix-to-fix step by LED count:", {k:f"p50 {pct(v,.5):.1f} p90 {pct(v,.9):.1f} mm n={len(v)}" for k,v in sorted(byled.items())})
    # transitions in the shown stream
    tj={}
    for a,b in zip(P,P[1:]):
        if b['t']-a['t']>30e6 or a['s']==0 or b['s']==0: continue
        d=1000*math.dist(a['p'],b['p'])
        k=f"{a['s']}->{b['s']}"
        tj.setdefault(k,[]).append(d)
    print("  shown step per sample (<=30 ms):", {k:f"p50 {pct(v,.5):.0f} p99 {pct(v,.99):.0f} mm, >50 mm: {sum(x>50 for x in v)} of {len(v)}" for k,v in sorted(tj.items())})
    # gyro drift: reacquire after >=0.3 s out of view: shown orientation just before vs the optical fix
    ot=[o['t'] for o in ok]; pt=[p['t'] for p in P]
    drift=[];pos=[]
    for i in range(1,len(ok)):
        gap=(ok[i]['t']-ok[i-1]['t'])/1e9
        if gap<0.3 or gap>4: continue
        j=bisect.bisect_right(pt,ok[i]['t'])-1
        if j<0 or P[j]['s']==0 or ok[i]['t']-P[j]['t']>40e6: continue
        drift.append((gap,ang(P[j]['q'],ok[i]['q']))); pos.append(1000*math.dist(P[j]['p'],ok[i]['p']))
    for lo,hi in [(0.3,1),(1,2),(2,4)]:
        v=[d for g,d in drift if lo<=g<hi]
        print(f"  out of view {lo}-{hi} s: orientation error at re-entry p50 {pct(v,.5):.1f} p90 {pct(v,.9):.1f} deg (n={len(v)})")
    # flips in the shown stream: orientation change > 90 deg between consecutive shown samples (<= 30 ms)
    flips=sum(1 for a,b in zip(P,P[1:]) if b['t']-a['t']<=30e6 and a['s']!=0 and b['s']!=0 and ang(a['q'],b['q'])>90)
    print(f"  shown orientation flips (>90 deg within 30 ms): {flips}")
    print(f"  position error at re-entry (hold vs optical) p50 {pct(pos,.5):.0f} p90 {pct(pos,.9):.0f} mm (n={len(pos)})")
    # turn rejections
    tr=[o for o in O if o['v']=='turn']
    print(f"  'turn' rejections {len(tr)}: LED count p50 {pct([o['leds'] for o in tr],.5)}, rms p50 {pct([o['rms'] for o in tr],.5):.2f} (accepted: LEDs p50 {pct([o['leds'] for o in ok],.5)}, rms {pct([o['rms'] for o in ok],.5):.2f})")
for p in sys.argv[1:]:
    analyse(p, p.split('/')[-1])
