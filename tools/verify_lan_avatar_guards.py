#!/usr/bin/env python3
"""Execute baked optional-avatar branches offline against null and valid objects."""
import struct,json,pathlib,hashlib
from unicorn import Uc,UC_ARCH_ARM64,UC_MODE_ARM,UC_HOOK_CODE
from unicorn.arm64_const import UC_ARM64_REG_X0,UC_ARM64_REG_X8,UC_ARM64_REG_X19,UC_ARM64_REG_X24,UC_ARM64_REG_S8,UC_ARM64_REG_SP,UC_ARM64_REG_LR
import argparse
a=argparse.ArgumentParser();a.add_argument('binary',type=pathlib.Path);a.add_argument('--report',type=pathlib.Path,required=True);args=a.parse_args()
p=args.binary;b=p.read_bytes();off=32;segments=[]
for _ in range(struct.unpack_from('<I',b,16)[0]):
 cmd,n=struct.unpack_from('<II',b,off)
 if cmd==0x19:
  name=b[off+8:off+24].split(b'\0')[0].decode();va,vs,fo,fs=struct.unpack_from('<QQQQ',b,off+24)
  if name.startswith('__NX0_'):segments.append((name,va,fo,fs))
 off+=n
base=next(va for n,va,_,_ in segments if n=='__NX0_X')
checks=[(0x810470,UC_ARM64_REG_X0,0x81047c,0x810478),(0x80f61c,UC_ARM64_REG_X24,0x80f62c,0x80f628),(0x810d4c,UC_ARM64_REG_X8,0x810edc,0x810d50),(0x3e1d5c,UC_ARM64_REG_X0,0x3e1da0,0x3e1d60)]
checks.extend([(0x818d00,UC_ARM64_REG_X0,None,0x818d04)])
checks.extend((site,UC_ARM64_REG_X0,site+4,site+4) for site in [0x3eeb5c,0x3eeb88,0x3f35c0])
results=[]
for site,reg,null,nonnull in checks:
 for present in [False,True]:
  u=Uc(UC_ARCH_ARM64,UC_MODE_ARM)
  for _,va,fo,fs in segments:
   u.mem_map(va,((fs+0xfff)//0x1000)*0x1000);u.mem_write(va,b[fo:fo+fs])
  obj=0x20000000;u.mem_map(obj,0x1000);u.mem_write(obj,struct.pack('<Q',obj+0x100));u.mem_write(obj+0x158,struct.pack('<Q',0x12345678));u.mem_write(obj+0x30,struct.pack('<Q',0x87654321))
  u.mem_write(obj+0x2f8,struct.pack('<I',7));u.reg_write(UC_ARM64_REG_SP,obj+0xf00);u.reg_write(UC_ARM64_REG_LR,obj+0x800)
  u.reg_write(reg,obj if present else 0);u.reg_write(UC_ARM64_REG_S8,0x3f800000)
  target=(obj+0x800) if not present and null is None else base+(nonnull if present else null);arrived=[]
  def hook(emu,addr,size,_):
   if addr==target:arrived.append(addr);emu.emu_stop()
  u.hook_add(UC_HOOK_CODE,hook);u.emu_start(base+site,0,count=15)
  assert arrived, (hex(site),present)
  if present:
   if site in [0x810470,0x80f61c]:assert u.reg_read(UC_ARM64_REG_X8)==0x12345678
   if site==0x80f61c:assert u.reg_read(UC_ARM64_REG_X0)==obj
   if site==0x810d4c:assert u.reg_read(UC_ARM64_REG_X19)==0x87654321
   if site==0x3e1d5c:assert u.reg_read(UC_ARM64_REG_S8)==0
  if site==0x818d00:assert u.reg_read(UC_ARM64_REG_SP)==obj+0xf00-(0x30 if present else 0)
  if site in [0x3eeb5c,0x3eeb88,0x3f35c0]:assert u.reg_read(UC_ARM64_REG_X8)==(7 if present else 0)
  results.append({'site':hex(site),'resource_present':present,'expected_continuation':('return_to_caller' if null is None and not present else hex(target-base)),'passed':True})
r={'runtime_sha256':hashlib.sha256(b).hexdigest(),'checks':results,'note':'Offline Unicorn checks of signed-iOS-build code paths only. Game execution is native ARM64.'}
args.report.write_text(json.dumps(r,indent=2)+'\n');print('16 null/non-null branch and register checks passed.')
