"""Research stdin validator. Only boolean evidence leaves the process."""
import base64
import ctypes
import hashlib
import json
import struct
import sys
from nacl.signing import VerifyKey
from nacl.bindings import crypto_aead_chacha20poly1305_decrypt
import xxhash
from verify_bootstrap_config import wire_fields


def decode(value):
    return base64.b64decode(value + '=' * (-len(value) % 4), altchars=b'-_', validate=True)


def verify(row):
    headers = row['headers']
    meta = decode(headers['x-request'])
    fields = {n: v for n, t, v in wire_fields(meta) if t == 0}
    VerifyKey(decode(headers['x-seed'])).verify(meta, decode(headers['x-hash']))
    body = decode(row['body'])
    binding = struct.pack('<QQQQ', row['request_id'], row['plain_hash'], row['global_tag'], 0x019c411fdeaf)
    aad = xxhash.xxh3_128_intdigest(binding).to_bytes(16, 'little')
    compressed = crypto_aead_chacha20poly1305_decrypt(body[32:] + body[:16], aad, body[16:24], decode(row['static_key']))
    lib = ctypes.CDLL('/opt/homebrew/opt/zstd/lib/libzstd.dylib')
    lib.ZSTD_getFrameContentSize.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
    lib.ZSTD_getFrameContentSize.restype = ctypes.c_uint64
    length = lib.ZSTD_getFrameContentSize(compressed, len(compressed))
    if length > 1024 * 1024:
        raise ValueError()
    out = ctypes.create_string_buffer(length)
    lib.ZSTD_decompress.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_size_t]
    lib.ZSTD_decompress.restype = ctypes.c_size_t
    if lib.ZSTD_decompress(out, length, compressed, len(compressed)) != length:
        raise ValueError()
    plaintext = out.raw[:length]
    content = json.loads(plaintext)
    nonce_material = xxhash.xxh3_128_intdigest(struct.pack('<Q', row['request_id']) + compressed).to_bytes(16, 'little')
    return {
        'signature': True, 'body_auth': True, 'json_object': isinstance(content, dict),
        'original_hash': xxhash.xxh3_64_intdigest(plaintext) == row['plain_hash'],
        'nonce_material': body[16:32] == nonce_material,
        'metadata_hash': fields.get(8, 0) == row['plain_hash'],
        'metadata_length': fields.get(7, 0) == len(plaintext),
        'metadata_request': fields.get(6, 0) == row['request_id'],
        'metadata_global': fields.get(1, 0) == row['global_tag'],
        'metadata_mode': fields.get(13, 0) == 1,
        'has_login_fields': isinstance(content, dict) and 'validateStr' in content and 'type' in content,
    }


if __name__ == '__main__':
    try:
        row = json.loads(sys.stdin.buffer.read(3 * 1024 * 1024))
        print(json.dumps(verify(row)))
    except Exception:
        print(json.dumps({'verification_failed': True}))
        sys.exit(1)
