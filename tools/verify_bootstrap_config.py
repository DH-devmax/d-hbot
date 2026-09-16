"""Verify the bundled 2.8.0 bootstrap public-key chain without executing the app.

Requires PyNaCl and lz4. Prints metadata only; configuration bytes remain in memory.
"""
import argparse
import base64
import hashlib
import re
import struct
import lz4.block
from urllib.parse import urlsplit
from nacl.signing import VerifyKey
from nacl.bindings import crypto_aead_chacha20poly1305_decrypt


def wire_fields(data):
    """Bounded wire inspection, preserving repeated and unknown fields."""
    position = 0

    def varint():
        nonlocal position
        value = 0
        for index in range(10):
            if position == len(data):
                raise ValueError('Truncated varint')
            byte = data[position]
            position += 1
            if index == 9 and byte > 1:
                raise ValueError('Varint overflow')
            value |= (byte & 127) << (index * 7)
            if byte < 128:
                return value
        raise ValueError('Varint overflow')

    fields = []
    while position < len(data):
        if len(fields) >= 256:
            raise ValueError('Field limit exceeded')
        tag = varint()
        number, kind = tag >> 3, tag & 7
        if not 0 < number < (1 << 29):
            raise ValueError('Invalid field number')
        if kind == 0:
            value = varint()
        elif kind in (1, 2, 5):
            length = varint() if kind == 2 else (8 if kind == 1 else 4)
            if length > len(data) - position:
                raise ValueError('Truncated field')
            value = data[position:position + length]
            position += length
        else:
            raise ValueError('Unsupported wire type')
        fields.append((number, kind, value))
    return fields


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('main_js')
    parser.add_argument('sample')
    args = parser.parse_args()
    with open(args.main_js, 'rb') as source:
        js = source.read()
    with open(args.sample, 'rb') as source:
        binary = source.read()
    if hashlib.sha256(js).hexdigest() != '97647260a119318da6897c66fb3441aa0f7d5d87514bc7e08e6a954f6d3494d5':
        raise SystemExit('Unsupported main script')
    if hashlib.sha256(binary).hexdigest() != '815b95a3357b2b22492a5a9fc51dc645e019023459155027a656d1902beb4a70':
        raise SystemExit('Unsupported native sample')
    matches = re.findall(rb'\.xinit\("([^"\\]+)"\)', js)
    if len(matches) != 1:
        raise SystemExit('Unexpected bootstrap call shape')
    parts = matches[0].split(b'.')
    if len(parts) != 3:
        raise SystemExit('Unexpected bootstrap part count')
    offset, root, context = 32, None, None
    for _ in range(struct.unpack_from('<I', binary, 16)[0]):
        command, size = struct.unpack_from('<II', binary, offset)
        if command == 25:
            address, _, file_offset, file_size = struct.unpack_from('<QQQQ', binary, offset + 24)
            if address <= 0x10028cb28 and 0x10028cb28 + 32 <= address + file_size:
                start = file_offset + 0x10028cb28 - address
                root = binary[start:start + 32]
                context = binary[start - 8:start]
        offset += size
    if root is None:
        raise SystemExit('Root public key mapping missing')
    record_keys = []
    for index, part in enumerate(parts[:2], 1):
        record = base64.b64decode(part + b'=' * (-len(part) % 4), altchars=b'-_', validate=True)
        if len(record) != 104:
            raise SystemExit('Unexpected signed record length')
        VerifyKey(root).verify(record[:-64], record[-64:])
        root = record[8:40]
        record_keys.append(root)
        print(f'PASS bundled public-key record {index}: signature verified, 104 bytes')
    envelope = base64.b64decode(parts[2] + b'=' * (-len(parts[2]) % 4), altchars=b'-_', validate=True)
    if len(envelope) < 96:
        raise SystemExit('Truncated configuration envelope')
    selector = struct.unpack_from('<Q', envelope)[0]
    selected = record_keys[0 if selector & 0x3c == 0 else 1]
    VerifyKey(selected).verify(envelope[:32], envelope[32:96])
    key = hashlib.blake2b(b'', digest_size=32, key=selected,
        salt=envelope[:8] + bytes(8), person=context + bytes(8)).digest()
    plaintext = crypto_aead_chacha20poly1305_decrypt(
        envelope[96:] + envelope[16:32], envelope[:8], envelope[8:16], key)
    print('PASS third part header signature and payload authentication; plaintext bytes:', len(plaintext))
    expected_size = (selector >> 10) & 0xfff
    if expected_size == 0:
        raise SystemExit('Invalid decompression size')
    decoded = lz4.block.decompress(plaintext, uncompressed_size=expected_size)
    if len(decoded) != expected_size:
        raise SystemExit('Decompressed size mismatch')
    print('PASS bounded LZ4 block decompression; bytes:', len(decoded))
    inner = wire_fields(decoded)
    print('Inner field shapes:', [(n, t, len(v) if isinstance(v, bytes) else 'integer') for n, t, v in inner])
    for number, kind, value in inner:
        if number in (6, 7, 8) and kind == 2:
            nested = wire_fields(value)
            print('Nested field', number, 'shapes:',
                [(n, t, len(v) if isinstance(v, bytes) else 'integer') for n, t, v in nested])
            if number in (7, 8):
                protocols = [v for n, t, v in nested if n == 4 and t == 0]
                # Native +0x162f8 selects this switch from descriptor field 4.
                schemes = {1: 'https', 2: 'http', 3: 'http', 4: 'https', 5: 'https', 6: 'https'}
                if len(protocols) != 1 or protocols[0] not in schemes:
                    raise SystemExit('Unknown connection protocol; do not infer a default')
                print('Connection configuration scheme:', schemes[protocols[0]])
            candidates = [(n, v) for n, t, v in nested if t == 2]
        elif number == 5 and kind == 2:
            candidates = [(number, value)]
        else:
            candidates = []
        for field, candidate in candidates:
            try:
                parsed = urlsplit(candidate.decode('utf-8'))
                if parsed.scheme in ('http', 'https', 'ws', 'wss') and parsed.hostname:
                    print('URL-shaped field:', number, field, 'scheme:', parsed.scheme,
                          'has_credentials:', parsed.username is not None or parsed.password is not None,
                          'has_query:', bool(parsed.query))
            except (UnicodeDecodeError, ValueError):
                pass
    print('Bootstrap SHA256:', hashlib.sha256(matches[0]).hexdigest())
    print('No login or network executed')


if __name__ == '__main__':
    main()
