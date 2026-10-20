#!/usr/bin/env python3
"""Verify the signed Mach-O projection/layout entries and relocated prologues."""
import argparse,struct,subprocess,json,hashlib
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('binary',type=Path);p.add_argument('--report',type=Path,required=True);a=p.parse_args()
b=a.binary.read_bytes();segments=[];o=32
for _ in range(struct.unpack_from('<I',b,16)[0]):
    cmd,n=struct.unpack_from('<II',b,o)
    if cmd==0x19:
        name=b[o+8:o+24].split(b'\0')[0].decode();va,vs,fo,fs=struct.unpack_from('<QQQQ',b,o+24);segments.append((name,va,fo,fs))
    o+=n
base=next(va for name,va,_,_ in segments if name=='__NX0_X')
def read(va,n):
    for _,start,fo,fs in segments:
        if start<=va and va+n<=start+fs:return b[fo+va-start:fo+va-start+n]
    raise ValueError(hex(va))
syms={parts[2]:int(parts[0],16) for l in subprocess.check_output(['nm','-g',str(a.binary)],text=True).splitlines() if len(parts:=l.split())==3 and parts[1]=='T'}
sites=[(0x61b8a4,'_mk8_projection_get',[0xa9be4ff4,0xa9017bfd,0x910043fd,0xaa0003f3]),(0x61b91c,'_mk8_projection_device_get',[0xf81d0ff5,0xa9014ff4,0xa9027bfd,0x910083fd]),(0x592808,'_mk8_pane_calculate',[0xfc1b0fe8,0xf90007f9,0xa9015ff8,0xa90257f6])]
results=[]
for i,(off,symbol,words) in enumerate(sites):
    pc=base+off;adrp,add,br,nop=struct.unpack('<4I',read(pc,16))
    assert adrp&0x9f00001f==0x90000010 and add&0xffc003ff==0x91000210 and br==0xd61f0200 and nop==0xd503201f
    imm=((adrp>>5)&0x7ffff)<<2 | ((adrp>>29)&3)
    if imm&0x100000:imm-=0x200000
    target=(pc&~0xfff)+(imm<<12)+((add>>10)&0xfff)
    assert target==syms[symbol],(symbol,hex(target),hex(syms[symbol]))
    stub=base+0x138f958+160+i*20
    assert list(struct.unpack('<4I',read(stub,16)))==words
    branch=struct.unpack('<I',read(stub+16,4))[0];assert branch&0xfc000000==0x14000000
    imm=branch&0x3ffffff
    if imm&0x2000000:imm-=0x4000000
    assert stub+16+imm*4==pc+16
    results.append({'entry':hex(off),'host':symbol,'entry_branch_matches':True,'original_prologue_matches':True,'continuation_matches':True})
r={'sha256':hashlib.sha256(b).hexdigest(),'checks':results,'note':'Static verification of signed executable. Runtime device checks are separate.'}
a.report.write_text(json.dumps(r,indent=2)+'\n');print('3 host redirects and 3 original-code continuation stubs verified.')
