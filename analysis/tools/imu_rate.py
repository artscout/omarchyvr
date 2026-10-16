import sys;sys.path.insert(0,'.')
import usbp,collections
p=usbp.read(sys.argv[1]); dev=int(sys.argv[2])
rows=[x for x in p if x['dev']==dev and x['ep']==0x83 and x['tt']==1 and len(x['data'])==64]
print(len(rows),rows[0]['t'],rows[-1]['t'],'rate',len(rows)/(rows[-1]['t']-rows[0]['t']))
for x in rows[:6]+rows[1000:1004]: print('%.5f'%x['t'],x['data'].hex(' '))
print(collections.Counter(x['data'][:2].hex() for x in rows))
