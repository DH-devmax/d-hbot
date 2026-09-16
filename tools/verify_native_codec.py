"""Offline arm64 decoder oracle. Requires rizin, Unicorn 2.x and PyNaCl 1.6.x.

No module initialization, account data or network calls. The caller supplies a
local sample; only the exact reviewed hash is accepted. Never commit the sample.
"""
import argparse, hashlib, subprocess, json, struct
from pathlib import Path
from unicorn import Uc,UC_ARCH_ARM64,UC_MODE_ARM,UC_HOOK_CODE
from unicorn.arm64_const import *
from nacl.bindings import crypto_aead_chacha20poly1305_encrypt
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("sample", type=Path)
args = parser.parse_args()
p = str(args.sample)
b=Path(p).read_bytes()
expected = "a92865e20618267bdeaff72cc2ed355d682e6a5248644a1a213cb47473598851"
if hashlib.sha256(b).hexdigest() != expected:
 raise SystemExit("Unsupported sample hash; static addresses must be reviewed first")
symbols=json.loads(subprocess.run(['rizin','-q','-c','isj',p],capture_output=True,text=True,check=True,timeout=60).stdout)
imports={s['vaddr']:s['name'] for s in symbols if s.get('name','').startswith('imp.')}
# Stubs also obtained as functions, to avoid executing any host library.
a=json.loads(subprocess.run(['rizin','-q','-c','aaa; aflj',p],capture_output=True,text=True,check=True,timeout=60).stdout)
imports.update({s['offset']:s['name'] for s in a if s.get('name','').startswith('sym.imp.')})
def probe(n,tamper=False):
 key=bytes(range(32));nonce=bytes(range(8));ad=b'DHPOC001';msg=bytes(i%251 for i in range(n))
 sealed=crypto_aead_chacha20poly1305_encrypt(msg,ad,nonce,key)
 ct,tag=sealed[:-16],sealed[-16:]
 if tamper:tag=bytes([tag[0]^1])+tag[1:]
 u=Uc(UC_ARCH_ARM64,UC_MODE_ARM);u.mem_map(0,0x80000);u.mem_write(0,b)
 u.mem_map(0x100000,0x40000);u.reg_write(UC_ARM64_REG_SP,0x13f000)
 # Synthetic stack protector only; no process loading or module initialization.
 u.mem_write(0x34050,struct.pack('<Q',0x70000));u.mem_write(0x70000,struct.pack('<Q',0x12345678))
 ptrs=[0x100000,0x104000,n,0x108000,0x109000,0x10a000,0x10b000]
 for ptr,data in [(ptrs[1],ct),(ptrs[3],tag),(ptrs[4],ad),(ptrs[5],nonce),(ptrs[6],key)]:
  if data:u.mem_write(ptr,data)
 for reg,val in zip([UC_ARM64_REG_X0,UC_ARM64_REG_X1,UC_ARM64_REG_X2,UC_ARM64_REG_X3,UC_ARM64_REG_X4,UC_ARM64_REG_X5,UC_ARM64_REG_X6],ptrs):u.reg_write(reg,val)
 u.reg_write(UC_ARM64_REG_LR,0x71000)
 def hook(u,addr,size,_):
  if addr not in imports:return
  name=imports[addr];x=[u.reg_read(r) for r in [UC_ARM64_REG_X0,UC_ARM64_REG_X1,UC_ARM64_REG_X2,UC_ARM64_REG_X3]]
  if name.endswith('memset_s'):
   assert x[3]<=x[1] and x[3]<100000
   u.mem_write(x[0],bytes([x[2]&255])*x[3]);u.reg_write(UC_ARM64_REG_X0,0)
  elif name.endswith('bzero'):
   if x[1]:u.mem_write(x[0],bytes(x[1]))
  else:raise RuntimeError('Unexpected external call '+name)
  u.reg_write(UC_ARM64_REG_PC,u.reg_read(UC_ARM64_REG_LR))
 u.hook_add(UC_HOOK_CODE,hook)
 u.emu_start(0x21c04,0x71000,count=1000000)
 assert u.reg_read(UC_ARM64_REG_PC)==0x71000
 result=u.reg_read(UC_ARM64_REG_X0)&0xffffffff
 out=bytes(u.mem_read(ptrs[0],n))
 assert result==(0xffffffff if tamper else 0),(n,tamper,result)
 assert out==(bytes(n) if tamper else msg),(n,tamper,'output mismatch')
 print('PASS',n,'tampered' if tamper else 'valid')
for n in [0,1,15,16,17,63,64,65,256,1024]:
 probe(n);probe(n,True)
