import sys, array
f=sys.argv[1]; out=sys.argv[2]; W,H=7104,3840; st=8
a=array.array('I'); a.fromfile(open(f,'rb'),W*H)
def px(x,y):
    v=a[y*W+x]; return (v&1023,(v>>10)&1023,(v>>20)&1023,v>>30)
rows=[]
for y in range(0,H,st):
    row=bytearray()
    for x in range(0,W,st):
        r,g,b,_=px(x,y); row+=bytes((r>>2,g>>2,b>>2))
    rows.append(bytes(row))
open(out,'wb').write(b'P6 %d %d 255\n'%(W//st,H//st)+b''.join(rows))
print(f.split('/')[-2:], 'left-eye bg', px(1000,1900), 'right bg', px(6000,1900), 'centre', px(1776,1920))
