"""Offline 2.8.0 arm64 signing oracle; synthetic keys only, no host execution.

Requires Unicorn, PyNaCl and xxhash. Unknown imports fail closed. Sample stays local.
"""
import argparse
import base64
import ctypes
import hashlib
import json
import itertools
import struct
import subprocess

from nacl.signing import SigningKey
from nacl.public import PrivateKey, SealedBox
from nacl.bindings import crypto_shorthash_siphash24
from nacl.bindings import crypto_aead_chacha20poly1305_encrypt
from nacl.bindings import crypto_sign_ed25519_pk_to_curve25519, crypto_sign_ed25519_sk_to_curve25519
import xxhash
import xxhash._xxhash
from unicorn import Uc, UC_ARCH_ARM64, UC_MODE_ARM, UC_HOOK_CODE
from unicorn.arm64_const import (UC_ARM64_REG_X19, UC_ARM64_REG_X20,
    UC_ARM64_REG_X21, UC_ARM64_REG_X22, UC_ARM64_REG_X25, UC_ARM64_REG_X26,
    UC_ARM64_REG_X0, UC_ARM64_REG_X1,
    UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4, UC_ARM64_REG_X5,
    UC_ARM64_REG_X6, UC_ARM64_REG_X7, UC_ARM64_REG_SP, UC_ARM64_REG_LR,
    UC_ARM64_REG_PC)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sample')
    parser.add_argument('--vectors', help='Write synthetic serializer vectors only')
    parser.add_argument('--sealed-vectors', help='Write synthetic sealed box vectors only')
    parser.add_argument('--zstd-library', help='Optional local libzstd for compression interoperability')
    args = parser.parse_args()
    with open(args.sample, 'rb') as source:
        binary = source.read()
    if hashlib.sha256(binary).hexdigest() != '815b95a3357b2b22492a5a9fc51dc645e019023459155027a656d1902beb4a70':
        raise SystemExit('Unsupported sample')
    symbols = json.loads(subprocess.run(['rizin', '-q', '-c', 'isj', args.sample],
        capture_output=True, text=True, check=True, timeout=60).stdout)
    imports = {s['vaddr']: s['name'] for s in symbols if s.get('name', '').startswith('imp.')}
    segments = []
    offset = 32
    for _ in range(struct.unpack_from('<I', binary, 16)[0]):
        command, size = struct.unpack_from('<II', binary, offset)
        if command == 0x19:
            address, virtual_size, file_offset, file_size = struct.unpack_from('<QQQQ', binary, offset+24)
            if file_size:
                segments.append((address, file_offset, file_size))
        offset += size
    for length in (0, 1, 31, 32, 63, 64, 127, 128, 129, 1024):
        machine = Uc(UC_ARCH_ARM64, UC_MODE_ARM)
        machine.mem_map(0x100000000, 0x400000)
        for address, file_offset, size in segments:
            machine.mem_write(address, binary[file_offset:file_offset+size])
        machine.mem_map(0x200000000, 0x100000)
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        machine.mem_write(0x100341b68, struct.pack('<Q', 0x2000f0000))
        seed = bytes(range(32))
        signer = SigningKey(seed)
        message = bytes(i % 251 for i in range(length))
        machine.mem_write(0x200020000, message or b'\0')
        machine.mem_write(0x200030000, seed + bytes(signer.verify_key))
        for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2, UC_ARM64_REG_X3),
                                  (0x200010000, 0x200020000, length, 0x200030000)):
            machine.reg_write(register, value)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)

        compression_heap = None

        def hook(uc, address, size, context):
            nonlocal compression_heap
            if address == 0x1002814f0:
                # Offline keygen only: replace RNG initialization with a
                # synthetic callback, never use host entropy or process state.
                uc.mem_write(0x10034ef98, struct.pack('<Q', 0x2000fe000))
                uc.reg_write(UC_ARM64_REG_PC, uc.reg_read(UC_ARM64_REG_LR))
                return
            if address == 0x2000fe000:
                if uc.reg_read(UC_ARM64_REG_X1) != 32:
                    raise RuntimeError('Unexpected synthetic RNG size')
                uc.mem_write(uc.reg_read(UC_ARM64_REG_X0), bytes(range(32)))
                uc.reg_write(UC_ARM64_REG_PC, uc.reg_read(UC_ARM64_REG_LR))
                return
            if address not in imports:
                return
            name = imports[address].removeprefix('imp.').lstrip('_')
            x0, x1, x2 = (uc.reg_read(r) for r in (UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2))
            if compression_heap is not None and name == 'malloc':
                allocation = (max(x0, 1) + 15) & ~15
                if allocation > 0x100000 or compression_heap + allocation > 0x301000000:
                    raise RuntimeError('Compression synthetic heap limit exceeded')
                uc.reg_write(UC_ARM64_REG_X0, compression_heap)
                compression_heap += allocation
            elif compression_heap is not None and name == 'free':
                if x0 and not 0x300000000 <= x0 < compression_heap:
                    raise RuntimeError('Invalid compression synthetic free')
            elif compression_heap is not None and name == 'memset' and x2 <= 0x100000:
                if x2:
                    uc.mem_write(x0, bytes([x1 & 255]) * x2)
            elif compression_heap is not None and name == 'bzero' and x1 <= 0x100000:
                if x1:
                    uc.mem_write(x0, bytes(x1))
            elif name in ('getuid', 'getgid'):
                uc.reg_write(UC_ARM64_REG_X0, 501)
            elif name in ('memcpy', 'memmove') and x2 <= 8192:
                if x2:
                    uc.mem_write(x0, bytes(uc.mem_read(x1, x2)))
            elif name == 'memset' and x2 <= 8192:
                if x2:
                    uc.mem_write(x0, bytes([x1 & 255]) * x2)
            elif name == 'bzero' and x1 <= 8192:
                if x1:
                    uc.mem_write(x0, bytes(x1))
            elif name == 'memset_s':
                count = uc.reg_read(UC_ARM64_REG_X3)
                if count > x1 or count > 8192:
                    raise RuntimeError('Invalid bounded memset_s')
                if count:
                    uc.mem_write(x0, bytes([x2 & 255]) * count)
                uc.reg_write(UC_ARM64_REG_X0, 0)
            else:
                raise RuntimeError('Unexpected external call: ' + name)
            uc.reg_write(UC_ARM64_REG_PC, uc.reg_read(UC_ARM64_REG_LR))

        machine.hook_add(UC_HOOK_CODE, hook)
        machine.emu_start(0x10027d87c, 0x2000ff000, count=10000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Instruction limit exceeded')
        result = bytes(machine.mem_read(0x200010000, 64))
        if result != signer.sign(message).signature:
            raise RuntimeError('Signature mismatch at synthetic length ' + str(length))
        print('PASS Ed25519 synthetic length', length)
        # The header builder calls this encoder with flags=7. Validate that
        # exact call shape rather than inferring an alphabet from its name.
        for payload in (message, result):
            if not payload:
                # The actual header wrapper skips the encoder for zero length.
                continue
            machine.mem_write(0x200020000, payload or b'\0')
            machine.mem_write(0x200040000, bytes(4096))
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                    UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4),
                    (0x200040000, 4096, 0x200020000, len(payload), 7)):
                machine.reg_write(register, value)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            machine.emu_start(0x100026fb8, 0x2000ff000, count=1000000)
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
                raise RuntimeError('Encoder instruction limit exceeded at ' + hex(machine.reg_read(UC_ARM64_REG_PC)))
            encoded = bytes(machine.mem_read(0x200040000, 4096)).split(b'\0', 1)[0]
            if encoded != base64.urlsafe_b64encode(payload).rstrip(b'='):
                raise RuntimeError('Header encoding mismatch')
        print('PASS header base64url: signature plus nonempty message cases; message length', length)

    machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
    machine.reg_write(UC_ARM64_REG_X0, 0x200090000)
    machine.reg_write(UC_ARM64_REG_X1, 0x200091000)
    machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
    machine.emu_start(0x10027d774, 0x2000ff000, count=10000000)
    if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
        raise RuntimeError('Keygen instruction limit exceeded')
    public = bytes(SigningKey(bytes(range(32))).verify_key)
    if bytes(machine.mem_read(0x200090000, 32)) != public:
        raise RuntimeError('Generated public key mismatch')
    if bytes(machine.mem_read(0x200091000, 64)) != bytes(range(32)) + public:
        raise RuntimeError('Generated secret key layout mismatch')
    print('PASS native keygen with synthetic RNG: public key and seed+public layout')

    for seed_byte in (0, 1, 127, 255):
        synthetic_seed = bytes([seed_byte]) * 32
        synthetic_public = bytes(SigningKey(synthetic_seed).verify_key)
        for entry, source, expected in (
            (0x10027fc84, synthetic_public,
             crypto_sign_ed25519_pk_to_curve25519(synthetic_public)),
            (0x100280f04, synthetic_seed + synthetic_public,
             crypto_sign_ed25519_sk_to_curve25519(synthetic_seed + synthetic_public)),
        ):
            machine.mem_write(0x200020000, source)
            machine.mem_write(0x200010000, bytes(32))
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            machine.reg_write(UC_ARM64_REG_X0, 0x200010000)
            machine.reg_write(UC_ARM64_REG_X1, 0x200020000)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            machine.emu_start(entry, 0x2000ff000, count=10000000)
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
                raise RuntimeError('Key conversion instruction limit exceeded')
            if machine.reg_read(UC_ARM64_REG_X0) != 0:
                raise RuntimeError('Valid synthetic key conversion rejected')
            if bytes(machine.mem_read(0x200010000, 32)) != expected:
                raise RuntimeError('Key conversion mismatch')
    print('PASS Ed25519 to X25519 public/secret conversions: 8 synthetic cases')

    recipient = PrivateKey(bytes(range(32)))
    if bytes(machine.mem_read(0x1002c6bc1, 14)) != b'v1/user/login\0':
        raise RuntimeError('Login route registration string mismatch')
    # Check the exact initialization registration rather than relying on a
    # potentially incomplete disassembler call graph.
    if bytes(machine.mem_read(0x100005358, 4)) != bytes.fromhex('63500691'):
        raise RuntimeError('Login callback address construction mismatch')
    print('PASS pinned login route registration: callback +0x19194')
    # Execute the call site up to the header builder, with synthetic context.
    # A nonzero exchange timestamp gates emission, not the ciphertext length.
    for timestamp in (0, 1):
        machine.mem_write(0x200050000, bytes(256))
        machine.mem_write(0x200050090, struct.pack('<QQ', 0x200060000, 80))
        machine.mem_write(0x2000500b8, struct.pack('<Q', timestamp))
        machine.reg_write(UC_ARM64_REG_X19, 0x200070000)
        machine.reg_write(UC_ARM64_REG_X20, 0x200050000)
        stop = 0x10002032c if timestamp else 0x10001e724
        machine.emu_start(0x10001e700, stop, count=20)
        if machine.reg_read(UC_ARM64_REG_PC) != stop:
            raise RuntimeError('Login context header branch mismatch')
        if timestamp:
            actual = tuple(machine.reg_read(register) for register in (
                UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2,
                UC_ARM64_REG_X3, UC_ARM64_REG_X4))
            if actual != (0x200070000, 9, 0x1002c6d60, 80, 0x200060000):
                raise RuntimeError('Login context header argument mismatch')
            if bytes(machine.mem_read(actual[2], 10)) != b'X-Context\0':
                raise RuntimeError('Login context header name mismatch')
    print('PASS login exchange X-Context header call: 2 synthetic branch cases')
    saved_body_key = bytes(machine.mem_read(0x10034b760, 32))
    synthetic_body_key = bytes(range(32))
    machine.mem_write(0x10034b760, synthetic_body_key)
    for length in (0, 1, 16, 65, 1024):
        plaintext = bytes(index % 251 for index in range(length))
        aad, nonce = bytes(range(16)), bytes(range(8))
        machine.mem_write(0x200020000, plaintext or b'\0')
        machine.mem_write(0x200030000, aad)
        machine.mem_write(0x200040000, nonce)
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4,
                UC_ARM64_REG_X5, UC_ARM64_REG_X6, UC_ARM64_REG_X7),
                (0x200010000, 0x200060000, 0x200070000, 0x200020000,
                 length, 0x200030000, len(aad), 0x200040000)):
            machine.reg_write(register, value)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x100272820, 0x2000ff000, count=10000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Business body encryption instruction limit exceeded')
        actual = bytes(machine.mem_read(0x200010000, length))
        actual += bytes(machine.mem_read(0x200060000, 16))
        if actual != crypto_aead_chacha20poly1305_encrypt(
                plaintext, aad, nonce, synthetic_body_key):
            raise RuntimeError('Business body encryption mismatch')
    machine.mem_write(0x10034b760, saved_body_key)
    print('PASS business body original ChaCha20-Poly1305 with synthetic global key: 5 cases')
    response_cases = 0
    for length in (0, 1, 16, 65, 1024):
        plaintext = bytes((index + 19) % 251 for index in range(length))
        aad, nonce, key = bytes(range(16)), bytes(range(8)), bytes(range(32))
        sealed = crypto_aead_chacha20poly1305_encrypt(plaintext, aad, nonce, key)
        for changed in ('none', 'key', 'aad', 'nonce', 'tag', 'ciphertext'):
            if changed == 'ciphertext' and not length:
                continue
            parts = {'key': key, 'aad': aad, 'nonce': nonce,
                     'tag': sealed[-16:], 'ciphertext': sealed[:-16]}
            if changed != 'none':
                damaged = bytearray(parts[changed])
                damaged[0] ^= 1
                parts[changed] = bytes(damaged)
            machine.mem_write(0x200010000, b'\xa5' * max(length, 1))
            for address, part in ((0x200020000, 'ciphertext'), (0x200030000, 'tag'),
                    (0x200040000, 'aad'), (0x200050000, 'nonce'), (0x200060000, 'key')):
                machine.mem_write(address, parts[part] or b'\0')
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                    UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4,
                    UC_ARM64_REG_X5, UC_ARM64_REG_X6, UC_ARM64_REG_X7),
                    (0x200010000, 0x200020000, length, 0x200030000,
                     0x200040000, 16, 0x200050000, 0x200060000)):
                machine.reg_write(register, value)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            machine.emu_start(0x100272a2c, 0x2000ff000, count=10000000)
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
                raise RuntimeError('Response decryption instruction limit exceeded')
            result = machine.reg_read(UC_ARM64_REG_X0) & 0xffffffff
            output = bytes(machine.mem_read(0x200010000, length))
            if changed == 'none':
                if result != 0 or output != plaintext:
                    raise RuntimeError('Independent response ciphertext rejected')
            elif result == 0 or output not in (bytes(length), b'\xa5' * length):
                raise RuntimeError('Response authentication failure exposed output')
            response_cases += 1
    print('PASS independent response decryption and key/AAD/nonce/tag/ciphertext tampering:', response_cases)
    for binding in ((0, 0, 0), (1, 2, 3), (0xffffffffffffffff, 17, 280)):
        machine.mem_write(0x200020000, bytes(33))
        machine.mem_write(0x200030000, bytes(32))
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4,
                UC_ARM64_REG_X5, UC_ARM64_REG_X6),
                (*binding, 1, 33, 0x200020000, 0x200030000)):
            machine.reg_write(register, value)
        boundary_hook = machine.hook_add(UC_HOOK_CODE,
            lambda uc, address, size, context: uc.emu_stop(),
            begin=0x100272a2c, end=0x100272a2c)
        try:
            machine.emu_start(0x10001f18c, 0x2000ff000, count=1000000)
        finally:
            machine.hook_del(boundary_hook)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x100272a2c:
            raise RuntimeError('Response authentication call site not reached')
        material = struct.pack('<QQQQ', *binding, 0x019c411fdeaf)
        expected_aad = xxhash.xxh3_128_intdigest(material).to_bytes(16, 'little')
        actual_aad = bytes(machine.mem_read(machine.reg_read(UC_ARM64_REG_X4), 16))
        actual_args = tuple(machine.reg_read(register) for register in (
            UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2,
            UC_ARM64_REG_X3, UC_ARM64_REG_X5, UC_ARM64_REG_X6, UC_ARM64_REG_X7))
        if actual_aad != expected_aad or actual_args != (
                0x200020020, 0x200020020, 1, 0x200020000, 16, 0x200020010, 0x10034b760):
            raise RuntimeError('Response authentication binding mismatch')
    print('PASS response mode 1 AAD binding and detached layout: 3 synthetic cases')
    for mode in (0, 1, 2):
        stack = 0x2000e0000
        machine.mem_write(stack + 0x48, struct.pack('<Q', 0x200050000))
        machine.mem_write(0x200050000, bytes(512))
        machine.mem_write(0x200050030, struct.pack('<QI', 0x200020000, 81))
        machine.mem_write(0x2000500a8, struct.pack('<Q', 17))
        machine.mem_write(0x200050160, struct.pack('<QI', 29, mode))
        machine.mem_write(0x200060168, struct.pack('<Q', 43))
        machine.reg_write(UC_ARM64_REG_SP, stack)
        machine.reg_write(UC_ARM64_REG_X25, 0x200060000)
        boundary_hook = machine.hook_add(UC_HOOK_CODE,
            lambda uc, address, size, context: uc.emu_stop(),
            begin=0x10001f18c, end=0x10001f18c)
        try:
            machine.emu_start(0x100017c10, 0x2000ff000, count=100)
        finally:
            machine.hook_del(boundary_hook)
        actual = tuple(machine.reg_read(register) for register in (
            UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2,
            UC_ARM64_REG_X3, UC_ARM64_REG_X4, UC_ARM64_REG_X5, UC_ARM64_REG_X6))
        if machine.reg_read(UC_ARM64_REG_PC) != 0x10001f18c or actual != (
                17, 29, 43, mode, 81, 0x200020000, 0x200050060):
            raise RuntimeError('Business response caller context mapping mismatch')
    print('PASS business response caller binding and mode propagation: 3 synthetic cases')
    for output_length in (32, 64):
        for identifier in (0, 1, 0xffffffffffffffff):
            context_bytes = b'SYNTHETC'
            synthetic_key = bytes(range(32))
            machine.mem_write(0x200020000, context_bytes)
            machine.mem_write(0x200030000, synthetic_key)
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                    UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4),
                    (0x200010000, output_length, identifier, 0x200020000, 0x200030000)):
                machine.reg_write(register, value)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            machine.emu_start(0x10027caac, 0x2000ff000, count=1000000)
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
                raise RuntimeError('Configuration KDF instruction limit exceeded')
            expected = hashlib.blake2b(b'', digest_size=output_length, key=synthetic_key,
                salt=struct.pack('<Q', identifier) + bytes(8),
                person=context_bytes + bytes(8)).digest()
            if bytes(machine.mem_read(0x200010000, output_length)) != expected:
                raise RuntimeError('Configuration KDF mismatch')
    print('PASS BLAKE2b KDF with identifier salt and context personalization: 6 synthetic cases')
    for length in (40, 72):
        message = bytes(i % 251 for i in range(length))
        signature = SigningKey(bytes(range(32))).sign(message).signature
        for tampered in (False, True):
            candidate = bytearray(message)
            if tampered:
                candidate[0] ^= 1
            machine.mem_write(0x200020000, bytes(candidate))
            machine.mem_write(0x200030000, signature)
            machine.mem_write(0x200040000, bytes(SigningKey(bytes(range(32))).verify_key))
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                    UC_ARM64_REG_X2, UC_ARM64_REG_X3),
                    (0x200030000, 0x200020000, length, 0x200040000)):
                machine.reg_write(register, value)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            machine.emu_start(0x10027e568, 0x2000ff000, count=10000000)
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
                raise RuntimeError('Configuration signature instruction limit exceeded')
            valid = (machine.reg_read(UC_ARM64_REG_X0) & 0xffffffff) == 0
            if valid == tampered:
                raise RuntimeError('Configuration Ed25519 verification mismatch')
    print('PASS configuration Ed25519 verification and tamper rejection: 4 synthetic cases')
    decode_cases = [(base64.urlsafe_b64encode(p).rstrip(b'='), p)
                    for p in (b'M', b'Ma', b'Man', bytes(range(255)))]
    decode_cases += [(b'TQ==', b'M'), (b'T W\nFu', b'Man'),
                     (b'TWFu!ignored', b'Man'), (b'TR', b''), (b'T', b'')]
    for encoded, payload in decode_cases:
        machine.mem_write(0x200020000, encoded)
        machine.mem_write(0x200080000, struct.pack('<QQ', len(encoded), 0x200020000 + len(encoded) - 1))
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4, UC_ARM64_REG_X5),
                (0x200010000, len(encoded), 0x200020000, len(encoded), 0x200080000, 0x200080008)):
            machine.reg_write(register, value)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x1000279e8, 0x2000ff000, count=1000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Response base64 instruction limit exceeded')
        size = struct.unpack('<Q', bytes(machine.mem_read(0x200080000, 8)))[0]
        if size != len(payload) or bytes(machine.mem_read(0x200010000, size)) != payload:
            raise RuntimeError('Response base64url mismatch')
    print('PASS response base64 decoding and malformed input behavior: 9 synthetic cases')
    sealed_vectors = []
    for length in (1, 16, 32, 128, 1024):
        plaintext = bytes(i % 251 for i in range(length))
        machine.mem_write(0x200020000, plaintext)
        machine.mem_write(0x200030000, bytes(recipient.public_key))
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                UC_ARM64_REG_X2, UC_ARM64_REG_X3),
                (0x200010000, 0x200020000, length, 0x200030000)):
            machine.reg_write(register, value)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x100273dec, 0x2000ff000, count=10000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Sealed box encryption instruction limit exceeded')
        if machine.reg_read(UC_ARM64_REG_X0) & 0xffffffff:
            raise RuntimeError('Native sealed box encryption failed')
        native_ciphertext = bytes(machine.mem_read(0x200010000, length + 48))
        if SealedBox(recipient).decrypt(native_ciphertext) != plaintext:
            raise RuntimeError('Native sealed box encryption interoperability mismatch')
        ciphertext = SealedBox(recipient.public_key).encrypt(plaintext)
        sealed_vectors.append({'plaintext_hex': plaintext.hex(), 'ciphertext_hex': ciphertext.hex()})
        for tampered in (False, True):
            candidate = bytearray(ciphertext)
            if tampered:
                candidate[-1] ^= 1
            machine.mem_write(0x200020000, bytes(candidate))
            machine.mem_write(0x200030000, bytes(recipient.public_key))
            machine.mem_write(0x200040000, bytes(recipient))
            machine.mem_write(0x200010000, bytes([0xa5]) * length)
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                    UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4),
                    (0x200010000, 0x200020000, len(candidate), 0x200030000, 0x200040000)):
                machine.reg_write(register, value)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            machine.emu_start(0x100274210, 0x2000ff000, count=10000000)
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
                raise RuntimeError('Sealed box instruction limit exceeded')
            result = machine.reg_read(UC_ARM64_REG_X0) & 0xffffffff
            output = bytes(machine.mem_read(0x200010000, length))
            if tampered:
                if result == 0 or output != bytes([0xa5]) * length:
                    raise RuntimeError('Sealed box tamper rejection mismatch')
            elif result != 0 or output != plaintext:
                raise RuntimeError('Sealed box interoperability mismatch')
    print('PASS sealed box native encryption: 5 synthetic cases; decryption and tamper rejection: 10')
    if args.sealed_vectors:
        with open(args.sealed_vectors, 'w') as target:
            json.dump({'sample_sha256': hashlib.sha256(binary).hexdigest(),
                'synthetic_private_key_hex': bytes(recipient).hex(),
                'public_key_hex': bytes(recipient.public_key).hex(),
                'vectors': sealed_vectors}, target, indent=2)
            target.write('\n')

    for variant in range(8):
        payload = bytes((i * 17 + variant * 31) % 256 for i in range(24))
        key = bytes((i * 13 + variant * 29) % 256 for i in range(16))
        machine.mem_write(0x200020000, payload)
        machine.mem_write(0x200030000, key)
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1, UC_ARM64_REG_X2),
                                  (0x200010000, 0x200020000, 0x200030000)):
            machine.reg_write(register, value)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x10027d510, 0x2000ff000, count=1000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Metadata MAC instruction limit exceeded')
        if bytes(machine.mem_read(0x200010000, 8)) != crypto_shorthash_siphash24(payload, key):
            raise RuntimeError('Metadata MAC mismatch')
    print('PASS metadata SipHash-2-4 over exactly 24 bytes: 8 synthetic cases')

    initialization_hash_mismatches = []
    class Hash128(ctypes.Structure):
        _fields_ = [('low64', ctypes.c_uint64), ('high64', ctypes.c_uint64)]
    library = ctypes.CDLL(xxhash._xxhash.__file__)
    with_secret = library.XXH3_128bits_withSecret
    with_secret.restype = Hash128
    with_secret.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_size_t]
    # Fixed binary table referenced by the 129..240 byte branch. It is sample
    # code data, not a credential read from an account or running process.
    fixed_table = ctypes.create_string_buffer(bytes(machine.mem_read(0x1002c69e0, 192)))
    fixed_table_mismatches = []
    for length in (0, 1, 3, 4, 8, 9, 16, 17, 32, 33, 64, 65, 96, 97, 128, 129, 240, 241, 1024):
        payload = bytes(i % 251 for i in range(length))
        machine.mem_write(0x200020000, payload or b'\0')
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        machine.reg_write(UC_ARM64_REG_X0, 0x200020000)
        machine.reg_write(UC_ARM64_REG_X1, length)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x100028efc, 0x2000ff000, count=1000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Buffer hash instruction limit exceeded')
        if machine.reg_read(UC_ARM64_REG_X0) != xxhash.xxh3_64_intdigest(payload):
            raise RuntimeError('Buffer hash mismatch at length ' + str(length))
        machine.reg_write(UC_ARM64_REG_X0, 0x200020000)
        machine.reg_write(UC_ARM64_REG_X1, length)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x10002951c, 0x2000ff000, count=1000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Body material hash instruction limit exceeded')
        body_digest = machine.reg_read(UC_ARM64_REG_X0) | (machine.reg_read(UC_ARM64_REG_X1) << 64)
        if body_digest != xxhash.xxh3_128_intdigest(payload):
            raise RuntimeError('Body material hash mismatch at length ' + str(length))
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        machine.reg_write(UC_ARM64_REG_X0, 0x200020000)
        machine.reg_write(UC_ARM64_REG_X1, length)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x100029ec8, 0x2000ff000, count=1000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Initialization hash instruction limit exceeded')
        digest = machine.reg_read(UC_ARM64_REG_X0) | (machine.reg_read(UC_ARM64_REG_X1) << 64)
        if digest != xxhash.xxh3_128_intdigest(payload):
            initialization_hash_mismatches.append(length)
        independent = with_secret(payload, length, fixed_table, 192)
        if digest != independent.low64 | (independent.high64 << 64):
            fixed_table_mismatches.append(length)
    print('PASS XXH3-64: 19 synthetic boundary lengths')
    print('PASS body material default XXH3-128: 19 synthetic boundary lengths')
    if args.zstd_library:
        zstd = ctypes.CDLL(args.zstd_library)
        decompress = zstd.ZSTD_decompress
        decompress.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_size_t]
        decompress.restype = ctypes.c_size_t
        compress = zstd.ZSTD_compress
        compress.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
        compress.restype = ctypes.c_size_t
        machine.mem_map(0x300000000, 0x1000000)
        # Rebase the compression strategy table from chained on-disk pointers.
        # These addresses are fixed to this sample, not runtime account data.
        for address in range(0x1003237e8, 0x100323970, 8):
            raw = struct.unpack('<Q', bytes(machine.mem_read(address, 8)))[0]
            target = raw & ((1 << 36) - 1)
            if target and target < 0x28c000:
                machine.mem_write(address, struct.pack('<Q', 0x100000000 + target))
        for length in (0, 1, 16, 65, 1024):
            compression_heap = 0x300000000
            payload = bytes(index % 251 for index in range(length))
            machine.mem_write(0x200020000, payload or b'\0')
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                    UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4),
                    (0x200010000, 8192, 0x200020000, length, 3)):
                machine.reg_write(register, value)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            try:
                machine.emu_start(0x100095c48, 0x2000ff000, count=10000000)
            except Exception as error:
                raise RuntimeError('Compression emulation failed at PC=' +
                    hex(machine.reg_read(UC_ARM64_REG_PC)) + ' LR=' +
                    hex(machine.reg_read(UC_ARM64_REG_LR))) from error
            size = machine.reg_read(UC_ARM64_REG_X0)
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000 or size > 8192:
                raise RuntimeError('Compression execution failed or exceeded limits')
            encoded = bytes(machine.mem_read(0x200010000, size))
            decoded = ctypes.create_string_buffer(max(length, 1))
            if decompress(decoded, length, encoded, size) != length or decoded.raw[:length] != payload:
                raise RuntimeError('Native compression Zstandard interoperability mismatch')
        compression_heap = None
        print('PASS native compression to independent Zstandard decompression: 5 cases')
        machine.mem_write(0x10034b760, synthetic_body_key)
        for length in (1, 16, 65, 1024):
            compression_heap = 0x300000000
            payload = bytes((index + 19) % 251 for index in range(length))
            compressed = ctypes.create_string_buffer(8192)
            size = compress(compressed, 8192, payload, length, 3)
            if size > 8192:
                raise RuntimeError('Independent compression failed')
            binding = (1, 2, 3)
            aad = xxhash.xxh3_128_intdigest(struct.pack(
                '<QQQQ', *binding, 0x019c411fdeaf)).to_bytes(16, 'little')
            nonce_material = bytes(range(16))
            encrypted = crypto_aead_chacha20poly1305_encrypt(
                compressed.raw[:size], aad, nonce_material[:8], synthetic_body_key)
            packet = encrypted[-16:] + nonce_material + encrypted[:-16]
            machine.mem_write(0x200020000, packet)
            machine.mem_write(0x200030000, struct.pack('<QII', 0x200040000, 0, 8192) + bytes(16))
            machine.mem_write(0x200040000, bytes(8192))
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            for register, value in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                    UC_ARM64_REG_X2, UC_ARM64_REG_X3, UC_ARM64_REG_X4,
                    UC_ARM64_REG_X5, UC_ARM64_REG_X6),
                    (*binding, 1, len(packet), 0x200020000, 0x200030000)):
                machine.reg_write(register, value)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            machine.emu_start(0x10001f18c, 0x2000ff000, count=10000000)
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
                raise RuntimeError('Full response decoding instruction limit exceeded')
            if machine.reg_read(UC_ARM64_REG_X0) != length:
                raise RuntimeError('Full response decoding length mismatch')
            if bytes(machine.mem_read(0x200040000, length)) != payload:
                raise RuntimeError('Full response decoding output mismatch')
        machine.mem_write(0x10034b760, saved_body_key)
        compression_heap = None
        print('PASS independent sealed Zstandard response through native mode 1 decoder: 4 cases')
    if fixed_table_mismatches:
        raise RuntimeError('Fixed-table XXH3-128 mismatch lengths: ' + str(fixed_table_mismatches))
    print('PASS fixed-table XXH3-128: 19 synthetic boundary lengths')
    print('Default-table XXH3-128 is incompatible at lengths:', initialization_hash_mismatches)

    for config, version in ((0, 0), (12, 280), (-12, -280)):
        sp = 0x2000c0000
        machine.mem_write(sp, bytes(4096))
        machine.mem_write(0x200050000, bytes(4096))
        machine.mem_write(sp + 0x48, struct.pack('<Q', 0x200050000))
        machine.mem_write(0x200060138, struct.pack('<i', config))
        machine.mem_write(0x10034f160, struct.pack('<i', version))
        machine.mem_write(0x200020000, b'synthetic-device')
        machine.mem_write(0x200030000, b'synthetic-config')
        for register, argument in ((UC_ARM64_REG_SP, sp), (UC_ARM64_REG_X26, sp+0x490),
                (UC_ARM64_REG_X25, 0x200060000), (UC_ARM64_REG_X19, 16),
                (UC_ARM64_REG_X20, 0x200030000), (UC_ARM64_REG_X21, 16),
                (UC_ARM64_REG_X22, 0x200020000)):
            machine.reg_write(register, argument)
        machine.emu_start(0x100004b24, 0x100004d18, count=1000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x100004d18:
            raise RuntimeError('Initialization input instruction limit exceeded')
        pointer, size = struct.unpack('<QI', bytes(machine.mem_read(sp+0x4d8, 12)))
        if size > 512:
            raise RuntimeError('Unexpected initialization input size')
        data = bytes(machine.mem_read(pointer, size))
        expected = b'01.\x04synthetic-devicesynthetic-config' + str(abs(config) % 10).encode() + str(abs(version)).encode() + b'2'
        if data != expected:
            raise RuntimeError('Synthetic initialization input mismatch: ' + data.hex())
    print('PASS initialization input fragment: zero, positive and negative numeric inputs')

    # Exercise the inner serializer with a preallocated output region. This
    # deliberately avoids inventing the native arena allocator or setjmp ABI.
    scalar_fields = ((2, 0), (3, 4), (4, 8), (5, 12), (7, 16), (12, 20), (13, 24))
    wide_fields = ((1, 32), (6, 40), (8, 48), (9, 56), (10, 64), (11, 72), (14, 80))
    # This DATA_CONST pointer uses Mach-O chained rebasing. Resolve only the
    # reviewed descriptor pointer (rizin's rebased view), not arbitrary data.
    machine.mem_write(0x1003203c0, struct.pack('<Q', 0x10028cef4))
    def varint(value):
        out = bytearray()
        while value >= 128:
            out.append((value & 127) | 128)
            value >>= 7
        out.append(value)
        return bytes(out)

    vectors = []
    for field, offset in scalar_fields + wide_fields + ((0, 0),):
        wide = (field, offset) in wide_fields
        values = (0, 1, 127, 128, 65535, 0x7fffffff, 0x80000000, 0xffffffff)
        if wide:
            values += (0x100000000, 0x7fffffffffffffff, 0x8000000000000000, 0xffffffffffffffff)
        for value in values:
            context, obj, start, end = 0x200050000, 0x200060008, 0x200070000, 0x200072000
            machine.mem_write(context, bytes(256))
            machine.mem_write(obj - 8, bytes(128))
            machine.mem_write(obj + offset, struct.pack('<Q' if wide else '<I', value))
            fields = [0] * 14
            if field:
                fields[field-1] = value
            else:
                for number, location in scalar_fields + wide_fields:
                    machine.mem_write(obj + location, struct.pack('<Q' if (number, location) in wide_fields else '<I', value))
                    fields[number-1] = value
            machine.mem_write(context + 0xc8, struct.pack('<QQQ', start, end, end))
            machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
            for register, argument in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                    UC_ARM64_REG_X2, UC_ARM64_REG_X3),
                    (context, obj, 0x1003203b8, 0x200080000)):
                machine.reg_write(register, argument)
            machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
            try:
                machine.emu_start(0x10025eddc, 0x2000ff000, count=1000000)
            except Exception as error:
                raise RuntimeError('Serializer stopped at ' + hex(machine.reg_read(UC_ARM64_REG_PC))) from error
            if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
                raise RuntimeError('Serializer instruction limit exceeded')
            position = struct.unpack('<Q', bytes(machine.mem_read(context + 0xd0, 8)))[0]
            if not start <= position <= end:
                raise RuntimeError('Serializer output outside preallocated buffer')
            wire = bytes(machine.mem_read(position, end-position)) if position < end else b''
            encoded_value = value
            if not wide and field != 13 and value & 0x80000000:
                encoded_value = value | 0xffffffff00000000
            expected = varint(field << 3) + varint(encoded_value) if value else b''
            if field == 0:
                expected = b''
                for number, location in sorted(scalar_fields + wide_fields):
                    v = value
                    if (number, location) in scalar_fields and number != 13 and value & 0x80000000:
                        v |= 0xffffffff00000000
                    if value:
                        expected += varint(number << 3) + varint(v)
            if wire != expected:
                raise RuntimeError(f'Synthetic field {field} value {value}: {wire.hex()} != {expected.hex()}')
            vectors.append({'fields': fields, 'wire_hex': wire.hex()})
        print('PASS synthetic scalar field', field)
    # The account record descriptor has a length-delimited field at object+24.
    machine.mem_write(0x100320408, struct.pack('<Q', 0x10028d038))
    for value in (0, 1, 127, 128, 0xffffffff, 0x100000000,
                  0x7fffffffffffffff, 0x8000000000000000, 0xffffffffffffffff):
        machine.mem_write(context, bytes(256))
        machine.mem_write(obj - 8, bytes(160))
        machine.mem_write(obj + 24, struct.pack('<Q', value))
        machine.mem_write(context + 0xc8, struct.pack('<QQQ', start, end, end))
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        for register, argument in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                UC_ARM64_REG_X2, UC_ARM64_REG_X3),
                (context, obj, 0x100320400, 0x200080000)):
            machine.reg_write(register, argument)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x10025eddc, 0x2000ff000, count=1000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('IPC binding serializer instruction limit exceeded')
        position = struct.unpack('<Q', bytes(machine.mem_read(context + 0xd0, 8)))[0]
        if not start <= position <= end:
            raise RuntimeError('IPC binding serializer output exceeded bounds')
        wire = bytes(machine.mem_read(position, end-position)) if position < end else b''
        if wire != (b'\x08' + varint(value) if value else b''):
            raise RuntimeError('IPC request binding field 1 mismatch')
    print('PASS IPC request binding field 1 uint64: 9 synthetic boundary cases')
    # Validate its wire identity without loading an actual persisted account.
    machine.mem_write(0x1003203a8, struct.pack('<Q', 0x10028ce94))
    machine.mem_write(0x1003203f0, struct.pack('<Q', 0x10028cff0))
    machine.mem_write(0x1003203d8, struct.pack('<Q', 0x10028cf9c))
    for (descriptor, field_offset, field_number), length in itertools.product(
            ((0x1003203a0, 24, 6), (0x1003203e8, 40, 4), (0x1003203d0, 24, 3)),
            (0, 1, 16, 32, 128)):
        payload = bytes(i % 251 for i in range(length))
        machine.mem_write(context, bytes(256))
        machine.mem_write(obj - 8, bytes(128))
        machine.mem_write(0x200020000, payload or b'\0')
        machine.mem_write(obj + field_offset, struct.pack('<QQ', 0x200020000, length))
        machine.mem_write(context + 0xc8, struct.pack('<QQQ', start, end, end))
        machine.reg_write(UC_ARM64_REG_SP, 0x2000e0000)
        for register, argument in zip((UC_ARM64_REG_X0, UC_ARM64_REG_X1,
                UC_ARM64_REG_X2, UC_ARM64_REG_X3),
                (context, obj, descriptor, 0x200080000)):
            machine.reg_write(register, argument)
        machine.reg_write(UC_ARM64_REG_LR, 0x2000ff000)
        machine.emu_start(0x10025eddc, 0x2000ff000, count=1000000)
        if machine.reg_read(UC_ARM64_REG_PC) != 0x2000ff000:
            raise RuntimeError('Account serializer instruction limit exceeded')
        position = struct.unpack('<Q', bytes(machine.mem_read(context + 0xd0, 8)))[0]
        if not start <= position <= end:
            raise RuntimeError('Account serializer output outside bounded buffer')
        wire = bytes(machine.mem_read(position, end-position)) if position < end else b''
        expected = varint(field_number << 3 | 2) + varint(length) + payload if length else b''
        if wire != expected:
            raise RuntimeError('Account/response bytes field wire mismatch')
    print('PASS account field 6, response field 4, login exchange field 3: 15 synthetic cases')
    if args.vectors:
        with open(args.vectors, 'w') as target:
            json.dump({'sample_sha256': hashlib.sha256(binary).hexdigest(), 'vectors': vectors}, target, indent=2)
            target.write('\n')


if __name__ == '__main__':
    main()
