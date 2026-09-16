"""Offline current native-message decoder oracle. Synthetic key and message only."""
import argparse
import base64
import hashlib
import json
import struct
import subprocess
from pathlib import Path

import lz4.block
from nacl.bindings import crypto_aead_chacha20poly1305_encrypt
from unicorn import Uc, UC_ARCH_ARM64, UC_MODE_ARM, UC_HOOK_CODE, UC_HOOK_MEM_INVALID
from unicorn.arm64_const import UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_SP, UC_ARM64_REG_LR, UC_ARM64_REG_PC


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sample", type=Path)
    parser.add_argument("--vector", type=Path, help="Synthetic Rust message vector JSON")
    args = parser.parse_args()
    binary = args.sample.read_bytes()
    if hashlib.sha256(binary).hexdigest() != "15d56b23104f7873a0ae167cbbb03a776af9906facef8f3f1f44a5c704e53de4":
        raise SystemExit("Unsupported sample hash")
    symbols = json.loads(subprocess.run(["rizin", "-q", "-c", "isj", str(args.sample)], capture_output=True, text=True, check=True, timeout=60).stdout)
    imports = {s["vaddr"]: s["name"].removeprefix("imp.").lstrip("_") for s in symbols if s["name"].startswith("imp.")}
    machine = Uc(UC_ARCH_ARM64, UC_MODE_ARM)
    machine.mem_map(0, 0x80000)
    offset = 32
    for _ in range(struct.unpack_from("<I", binary, 16)[0]):
        command, size = struct.unpack_from("<II", binary, offset)
        if command == 0x19:
            address, _, file_offset, file_size = struct.unpack_from("<QQQQ", binary, offset+24)
            if file_size:
                machine.mem_write(address, binary[file_offset:file_offset+file_size])
        offset += size
    # Apply reviewed Mach-O chained rebases to local descriptor pointers.
    relocations = json.loads(subprocess.run(["rizin", "-q", "-c", "irj", str(args.sample)], capture_output=True, text=True, check=True, timeout=60).stdout)
    for relocation in relocations:
        address = relocation["vaddr"]
        value = struct.unpack("<Q", bytes(machine.mem_read(address, 8)))[0]
        if not value >> 63:
            target = value & ((1 << 36) - 1)
            if target < 0x80000:
                machine.mem_write(address, struct.pack("<Q", target))
    # Synthetic stack protector pointer for string-bearing protobuf decoding.
    machine.mem_write(0x2c230, struct.pack("<Q", 0x70000))
    machine.mem_write(0x70000, struct.pack("<Q", 0x12345678))
    machine.mem_map(0x100000, 0x400000)
    machine.reg_write(UC_ARM64_REG_SP, 0x4f0000)
    # Replace only the in-emulator key; never modify the installed binary.
    machine.mem_write(0x30010, bytes([7])*32)
    heap = 0x200000
    allocations = {}

    def hook(uc, address, size, context):
        nonlocal heap
        if address not in imports:
            return
        name = imports[address]
        x0, x1, x2, x3 = [uc.reg_read(r) for r in (UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2, UC_ARM64_REG_X3)]
        if name in ("malloc", "calloc", "realloc"):
            count = x1 if name == "realloc" else x0 if name == "malloc" else x0*x1
            if count > 0x100000 or heap+count >= 0x400000:
                raise RuntimeError("Synthetic heap limit")
            uc.reg_write(UC_ARM64_REG_X0, heap)
            if name == "realloc" and x0:
                old_size = allocations.pop(x0)
                if min(old_size, count):
                    uc.mem_write(heap, bytes(uc.mem_read(x0, min(old_size, count))))
            allocations[heap] = count
            heap += (max(count, 1)+15)&~15
        elif name == "setjmp":
            uc.reg_write(UC_ARM64_REG_X0, 0)
        elif name == "free":
            pass
        elif name in ("memcpy", "memmove", "memcpy_chk") and x2 <= 0x100000:
            if x2:
                uc.mem_write(x0, bytes(uc.mem_read(x1, x2)))
        elif name in ("memset", "bzero", "memset_s"):
            count, value = (x1, 0) if name == "bzero" else (x3, x2) if name == "memset_s" else (x2, x1)
            if count > 0x100000:
                raise RuntimeError("Synthetic memset limit")
            if count:
                uc.mem_write(x0, bytes([value & 255])*count)
            if name == "memset_s":
                uc.reg_write(UC_ARM64_REG_X0, 0)
        else:
            raise RuntimeError("Unexpected import: " + name)
        uc.reg_write(UC_ARM64_REG_PC, uc.reg_read(UC_ARM64_REG_LR))

    machine.hook_add(UC_HOOK_CODE, hook)
    def invalid(uc, access, address, size, value, context):
        print(f"Unmapped emulation access at PC={uc.reg_read(UC_ARM64_REG_PC):x}, address={address:x}")
        return False
    machine.hook_add(UC_HOOK_MEM_INVALID, invalid)
    plain = bytes.fromhex("0a02080c1202082220012802a2060a0a084044482074657374")
    compressed = lz4.block.compress(plain, store_size=True)
    sealed = crypto_aead_chacha20poly1305_encrypt(compressed, struct.pack("<Q", 9), struct.pack("<Q", 8), bytes([7])*32)
    envelope = b"\x09"+struct.pack("<Q", (12<<34)|(34<<4)|(2<<2)|1)+b"\x19"+struct.pack("<Q",8)+b"\x22"+bytes([len(sealed)-16])+sealed[:-16]+b"\x30\x09\x3a\x10"+sealed[-16:]
    payload = json.dumps({"b":base64.urlsafe_b64encode(envelope).decode().rstrip("=")}, separators=(",", ":")).encode()
    if args.vector:
        vector = json.loads(args.vector.read_text())
        plain = base64.b64decode(vector["plaintext"], validate=True)
        payload = vector["payload"].encode()
        if len(plain) > 4096 or len(payload) > 8192:
            raise RuntimeError("Synthetic vector limit")
    machine.mem_write(0x100000, payload)
    for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2), (0x100000,len(payload),0x110000)):
        machine.reg_write(register, value)
    machine.reg_write(UC_ARM64_REG_LR, 0x120000)
    machine.emu_start(0x2c2c, 0x120000, count=10000000)
    if machine.reg_read(UC_ARM64_REG_PC) != 0x120000:
        raise RuntimeError("Instruction limit")
    result = machine.reg_read(UC_ARM64_REG_X0)
    if result != len(plain) or bytes(machine.mem_read(0x110000,len(plain))) != plain:
        raise RuntimeError("Native decoder mismatch: " + str(result))
    print("PASS native authenticated message envelope: synthetic protobuf, LZ4, routing binding")


if __name__ == "__main__":
    main()
