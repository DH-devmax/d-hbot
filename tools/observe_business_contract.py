"""LLDB research: validate complete signed requests in memory, emit booleans only.

Usage: dh-business SECONDS. Never stores credentials, payloads, or header values.
"""
import base64
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import time
import lldb

EXPECTED = '815b95a3357b2b22492a5a9fc51dc645e019023459155027a656d1902beb4a70'
ROOT = Path(__file__).resolve().parent
BASE = 0
SEEN = 0


def read(process, address, size):
    if not 0 <= size <= 1024 * 1024:
        raise ValueError()
    error = lldb.SBError()
    value = process.ReadMemory(address, size, error)
    if error.Fail() or len(value) != size:
        raise ValueError()
    return value


def number(process, address, size=8):
    return int.from_bytes(read(process, address, size), 'little')


def hit(frame, bp, internal):
    global SEEN
    try:
        p = frame.GetThread().GetProcess()
        request = frame.FindRegister('x19').GetValueAsUnsigned()
        context = frame.FindRegister('x20').GetValueAsUnsigned()
        if number(p, context + 0x168, 4) != 1:
            return False
        headers = {}
        node = number(p, request + 0xe0)
        for _ in range(32):
            if not node:
                break
            ptr = number(p, node)
            error = lldb.SBError()
            line = p.ReadCStringFromMemory(ptr, 8192, error)
            if error.Fail():
                raise ValueError()
            name, sep, value = line.partition(':')
            if sep:
                headers[name.lower()] = value.strip()
            node = number(p, node + 8)
        if not all(name in headers for name in ('x-request', 'x-seed', 'x-hash')):
            raise ValueError()
        body = read(p, number(p, request + 0xd0), number(p, request + 0xc8))
        row = {'headers': headers, 'body': base64.b64encode(body).decode(),
               'request_id': number(p, context + 0xa8),
               'plain_hash': number(p, context + 0x160),
               'global_tag': number(p, BASE + 0x34f168),
               'static_key': base64.b64encode(read(p, BASE + 0x34b760, 32)).decode()}
        child = subprocess.run(['/tmp/dh-sign-oracle-env/bin/python', str(ROOT / 'verify_live_business.py')],
                               input=json.dumps(row), capture_output=True, text=True, timeout=3)
        evidence = json.loads(child.stdout)
        if not isinstance(evidence, dict) or any(type(v) is not bool for v in evidence.values()):
            raise ValueError()
        evidence['exchange_present'] = bool(headers.get('x-context'))
        print(json.dumps({'business_contract': evidence}), flush=True)
        SEEN += 1
    except Exception:
        print('business_contract_observation_failed', flush=True)
    return False


def run(debugger, command, result, internal):
    global BASE, SEEN
    seconds = int(command or '60')
    if not 1 <= seconds <= 600:
        raise ValueError('Duration must be 1..600 seconds')
    target = debugger.GetSelectedTarget()
    process = target.GetProcess()
    previous = debugger.GetAsync()
    ids = []
    SEEN = 0
    try:
        path = Path(str(target.GetExecutable()))
        if path.name != 'mclient' or hashlib.sha256(path.read_bytes()).hexdigest() != EXPECTED:
            raise ValueError('Unsupported sample')
        module = target.FindModule(target.GetExecutable())
        BASE = module.GetObjectFileHeaderAddress().GetLoadAddress(target)
        if read(process, BASE + 0x1efcc, 4) != bytes.fromhex('886a41b9'):
            raise ValueError('Instruction mismatch')
        bp = target.BreakpointCreateByAddress(BASE + 0x1efcc)
        ids.append(bp.GetID())
        bp.SetScriptCallbackFunction('observe_business_contract.hit')
        if not bp.GetNumLocations():
            raise ValueError('No breakpoint location')
        debugger.SetAsync(True)
        if process.Continue().Fail():
            raise ValueError('Cannot resume')
        print('business_contract_probe_ready', flush=True)
        start = time.monotonic()
        while time.monotonic() - start < seconds:
            if process.GetState() in (lldb.eStateExited, lldb.eStateDetached, lldb.eStateInvalid):
                break
            time.sleep(0.1)
    finally:
        process.Stop()
        for ident in ids:
            target.BreakpointDelete(ident)
        detached = process.Detach().Success()
        debugger.SetAsync(previous)
        print(json.dumps({'business_contract_observed': SEEN, 'detached': detached}), flush=True)


def __lldb_init_module(debugger, internal):
    debugger.HandleCommand('command script add -f observe_business_contract.run dh-business')
