from pathlib import Path
import struct
root=Path(__file__).parent
base=(root/'opus-r128-comment.ogg').read_bytes()
def pages(data):
 out=[]; pos=0
 while pos<len(data):
  assert data[pos:pos+4]==b'OggS'
  n=data[pos+26]; segs=data[pos+27:pos+27+n]; size=sum(segs)
  out.append(data[pos:pos+27+n+size]); pos+=27+n+size
 return out
def crc(data):
 value=0
 for byte in data:
  value ^= byte<<24
  for _ in range(8): value=((value<<1)^0x04c11db7)&0xffffffff if value&0x80000000 else (value<<1)&0xffffffff
 return value
def rebuild_comment_page(old, packet):
 assert len(packet)<255
 header=bytearray(old[:27]); header[26]=1
 page=header+bytes([len(packet)])+packet
 page[22:26]=b'\0'*4
 page[22:26]=struct.pack('<I',crc(page))
 return bytes(page)
def tags(comments):
 vendor=b'Lavf63.1.102'
 packet=b'OpusTags'+struct.pack('<I',len(vendor))+vendor+struct.pack('<I',len(comments))
 for c in comments:
  b=c.encode(); packet+=struct.pack('<I',len(b))+b
 return packet
p=pages(base)
assert len(p)==3
for name, comments in {
 'opus-r128-single.ogg':['encoder=fixture','R128_TRACK_GAIN=1536'],
 'opus-r128-duplicate-identical.ogg':['encoder=fixture','R128_TRACK_GAIN=1536','R128_TRACK_GAIN=1536'],
 'opus-r128-duplicate-conflicting.ogg':['encoder=fixture','R128_TRACK_GAIN=1536','R128_TRACK_GAIN=100'],
 'opus-r128-invalid-whitespace.ogg':['encoder=fixture','R128_TRACK_GAIN= 1536'],
}.items():
 out=p[0]+rebuild_comment_page(p[1],tags(comments))+p[2]
 (root/name).write_bytes(out)
 print(name,len(out),crc(p[1]) if False else 'crc-ok')
