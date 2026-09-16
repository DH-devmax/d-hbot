"""Find direct ARM64 B/BL references in the pinned 2.8.0 Mach-O sample.

Only code addresses are printed; never attaches to a running account process.
This supplements disassembler analysis which can omit calls inside large functions.
"""
import argparse
import hashlib
import struct


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('sample')
    parser.add_argument('target', type=lambda value: int(value, 0))
    parser.add_argument('--page', action='store_true',
                        help='Find ADRP references to the target page instead of branches')
    args = parser.parse_args()
    with open(args.sample, 'rb') as source:
        binary = source.read()
    if hashlib.sha256(binary).hexdigest() != '815b95a3357b2b22492a5a9fc51dc645e019023459155027a656d1902beb4a70':
        raise SystemExit('Unsupported sample')
    offset = 32
    for _ in range(struct.unpack_from('<I', binary, 16)[0]):
        command, size = struct.unpack_from('<II', binary, offset)
        if command == 0x19:
            sections = struct.unpack_from('<I', binary, offset + 64)[0]
            for index in range(sections):
                section = offset + 72 + index * 80
                if binary[section:section + 16].rstrip(b'\0') != b'__text':
                    continue
                address, length, file_offset = struct.unpack_from('<QQI', binary, section + 32)
                for relative in range(0, length - 3, 4):
                    word = struct.unpack_from('<I', binary, file_offset + relative)[0]
                    site = address + relative
                    if args.page:
                        if word & 0x9f000000 != 0x90000000:
                            continue
                        immediate = ((word >> 5) & 0x7ffff) << 2 | ((word >> 29) & 3)
                        if immediate & 0x100000:
                            immediate -= 0x200000
                        if (site & ~0xfff) + (immediate << 12) == args.target & ~0xfff:
                            print(hex(site), 'ADRP', 'x' + str(word & 31), hex(args.target & ~0xfff))
                        continue
                    if word & 0x7c000000 != 0x14000000:
                        continue
                    immediate = word & 0x3ffffff
                    if immediate & 0x2000000:
                        immediate -= 0x4000000
                    if site + immediate * 4 == args.target:
                        print(hex(site), 'BL' if word >> 31 else 'B', hex(args.target))
        offset += size


if __name__ == '__main__':
    main()
