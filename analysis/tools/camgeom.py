import math, re, sys
import xml.etree.ElementTree as ET
root = ET.parse(sys.argv[1]).getroot()
cams = []
for c in root.findall('Camera'):
    cal = c.find('Calibration'); rig = c.find('Rig')
    pp = list(map(float, cal.get('principal_point').split()))
    fl = list(map(float, cal.get('focal_length').split()))
    k = list(map(float, cal.get('radial_distortion').split()))
    t = list(map(float, rig.get('translation').split()))
    R = list(map(float, rig.get('rowMajorRotationMat').split()))
    R = [R[0:3], R[3:6], R[6:9]]
    cams.append(dict(name=c.get('name'), pp=pp, f=fl, k=k, t=t, R=R))
def thd(th, k): # Kannala-Brandt
    s = 1.0; p = th*th
    for i, kk in enumerate(k[:4]): s += kk * p**(i+1)
    return th*s
def inv(rn, k):
    lo, hi = 0.0, 1.6
    for _ in range(60):
        m = (lo+hi)/2
        if thd(m, k) < rn: lo = m
        else: hi = m
    return lo
for c in cams:
    f = sum(c['f'])/2
    h = inv(320/f, c['k']); v = inv(240/f, c['k']); va = inv(233/f, c['k'])
    print(f"{c['name']:15s} f={f:.1f}  HFOV={2*math.degrees(h):.0f}  VFOV(480)={2*math.degrees(v):.0f}  VFOV(466 active)={2*math.degrees(va):.0f}  monotonic_to={max(t for t in [x/100 for x in range(1,160)] if all(thd((t2+1)/100,c['k'])>thd(t2/100,c['k']) for t2 in range(0,int(t*100))))*57.3:.0f}deg")
# Convention 2: R maps cam0 coords -> cam_i coords, t likewise; centre_i in cam0 = -R^T t; axes_i in cam0 = rows of R
def mT(R): return [[R[j][i] for j in range(3)] for i in range(3)]
def mv(R, v): return [sum(R[i][j]*v[j] for j in range(3)) for i in range(3)]
def dot(a,b): return sum(x*y for x,y in zip(a,b))
def norm(a): n=math.sqrt(dot(a,a)); return [x/n for x in a]
for conv in (1, 2):
    print(f"--- convention {conv} ({'R,t = pose of cam_i in cam0 (cam_i->cam0)' if conv==1 else 'R,t map cam0 -> cam_i'})")
    P = []
    for c in cams:
        if conv == 1:
            ctr = c['t']; z = [c['R'][i][2] for i in range(3)]; Rc2r = c['R']
        else:
            Rt = mT(c['R']); ctr = [-x for x in mv(Rt, c['t'])]; z = c['R'][2]; Rc2r = Rt
        P.append((c['name'], ctr, z, Rc2r))
        print(f"  {c['name']:15s} centre(mm)=({ctr[0]*1000:7.1f},{ctr[1]*1000:7.1f},{ctr[2]*1000:7.1f})  axis=({z[0]:.3f},{z[1]:.3f},{z[2]:.3f})")
    for i in range(4):
        for j in range(i+1,4):
            a = math.degrees(math.acos(max(-1,min(1,dot(P[i][2],P[j][2])))))
            b = math.dist(P[i][1],P[j][1])*1000
            print(f"    {P[i][0]}-{P[j][0]}: axes {a:.0f} deg apart, baseline {b:.0f} mm")
    # overlap: fraction of sphere directions seen by both cameras of a pair (ignore baseline), using 640x466 image bounds
    def sees(cam, Rc2r, d):
        # d in cam0 frame -> cam frame: Rr2c = Rc2r^T
        dc = mv(mT(Rc2r), d)
        th = math.acos(max(-1,min(1,dc[2])))
        if th > math.radians(80): return False
        r = sum(cam['f'])/2 * thd(th, cam['k'])
        phi = math.atan2(dc[1], dc[0])
        u = cam['pp'][0] + r*math.cos(phi); v = cam['pp'][1] + r*math.sin(phi)
        return 0 <= u < 640 and 7 <= v < 473
    N = 20000; dirs = []
    ga = math.pi*(3-math.sqrt(5))
    for n in range(N):
        y = 1 - 2*(n+0.5)/N; rr = math.sqrt(1-y*y); th = ga*n
        dirs.append([math.cos(th)*rr, y, math.sin(th)*rr])
    vis = [[sees(cams[i], P[i][3], d) for d in dirs] for i in range(4)]
    for i in range(4):
        print(f"    solid angle {cams[i]['name']}: {sum(vis[i])/N*4*math.pi:.2f} sr")
    for (i,j) in [(0,1),(2,3),(0,2),(1,3)]:
        both = sum(1 for n in range(N) if vis[i][n] and vis[j][n])
        print(f"    overlap {cams[i]['name']}+{cams[j]['name']}: {both/N*4*math.pi:.2f} sr ({both/N*100:.1f}% of sphere)")
    anyc = sum(1 for n in range(N) if any(vis[i][n] for i in range(4)))
    two = sum(1 for n in range(N) if sum(vis[i][n] for i in range(4))>=2)
    print(f"    union all 4: {anyc/N*100:.1f}% of sphere; seen by >=2 cams: {two/N*100:.1f}%")
