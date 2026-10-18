#!/usr/bin/env python3
"""Read-only, explicit-PID process-tree sample. No command lines or user data."""
import argparse,hashlib,json,os,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--state',required=True);p.add_argument('--output',required=True);p.add_argument('--samples',type=int,default=10);p.add_argument('--interval',type=float,default=1);a=p.parse_args()
if not 2<=a.samples<=60 or not .1<=a.interval<=5:raise SystemExit('Sampling bounds invalid')
s=json.loads(Path(a.state).read_text());root=int(s['pid']);expected=s['binary']
if os.readlink(f'/proc/{root}/exe')!=expected:raise SystemExit('Selected app executable changed')
if hashlib.sha256(Path(expected).read_bytes()).hexdigest()!=s.get('sha256'):raise SystemExit('Selected executable hash changed')
ticks=os.sysconf('SC_CLK_TCK');pages=os.sysconf('SC_PAGE_SIZE');start_identity=Path(f'/proc/{root}/stat').read_text().rsplit(')',1)[1].split()[19]
def capture():
 rows={}
 for path in Path('/proc').iterdir():
  if not path.name.isdigit():continue
  try:
   fields=(path/'stat').read_text().rsplit(')',1)[1].split();rows[int(path.name)]={'parent':int(fields[1]),'cpu':(int(fields[11])+int(fields[12]))/ticks,'rss':int(fields[21])*pages,'born':fields[19],'path':path}
  except (OSError,ValueError,IndexError):pass
 selected={root};changed=True
 while changed:
  children={pid for pid,r in rows.items() if r['parent'] in selected};changed=bool(children-selected);selected|=children
 if root not in rows or rows[root]['born']!=start_identity:raise SystemExit('Selected process exited or changed')
 totals={'atSeconds':time.monotonic(),'processCount':len(selected),'rssBytes':0,'pssBytes':0,'pssComplete':True,'readBytes':0,'writeBytes':0,'cpuSeconds':0,'cpuByProcess':{}}
 for pid in selected:
  row=rows.get(pid)
  if not row:continue
  totals['rssBytes']+=row['rss'];totals['cpuSeconds']+=row['cpu'];totals['cpuByProcess'][f"{pid}:{row['born']}"]=row['cpu']
  try:
   mem=dict(line.split(':',1) for line in (row['path']/'smaps_rollup').read_text().splitlines() if ':'in line);totals['pssBytes']+=int(mem['Pss'].split()[0])*1024
  except (OSError,KeyError,ValueError):totals['pssComplete']=False
  try:
   io=dict(line.split(':',1) for line in (row['path']/'io').read_text().splitlines());totals['readBytes']+=int(io['read_bytes']);totals['writeBytes']+=int(io['write_bytes'])
  except (OSError,KeyError,ValueError):pass
 return totals
samples=[];previous=None;epoch=None
for i in range(a.samples):
 row=capture();now=row.pop('atSeconds');epoch=now if epoch is None else epoch;row['elapsedSeconds']=round(now-epoch,3)
 if previous:
  dt=now-previous[0];row['cpuPercentOneCore']=round(100*sum(max(0,cpu-previous[1].get(key,cpu)) for key,cpu in row['cpuByProcess'].items())/dt,2)
 previous=(now,row.pop('cpuByProcess'));samples.append(row)
 if i+1<a.samples:time.sleep(a.interval)
out={'scope':'Selected app and descendant process snapshot only; development build, no isolation or steady-playback qualification. RSS sum may double-count shared pages; PSS provided when available. CPU excludes newly appearing and departed child lifetimes. I/O is kernel storage bytes, not network bytes.','binarySha256':s.get('sha256'),'samples':samples}
Path(a.output).write_text(json.dumps(out,indent=2)+'\n');print(json.dumps(out))
