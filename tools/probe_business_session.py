"""LLDB one-shot handoff to independent Rust. No account bytes are persisted."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading
import time
import uuid
from urllib.parse import urlsplit
import lldb
from observe_business_contract import read, number, EXPECTED

ROOT = Path(__file__).resolve().parent.parent
BASE = 0
MACS = {}
WORKER = None
COMPLETE = False
LOGIN = False
WEB = False


def account_hit(frame, bp, internal):
    try:
        p = frame.GetThread().GetProcess()
        account = frame.FindRegister('x0').GetValueAsUnsigned()
        context = frame.FindRegister('x20').GetValueAsUnsigned()
        record = number(p, account + 8)
        if len(MACS) < 64:
            MACS[context] = read(p, number(p, record + 0x18), 16)
    except Exception:
        pass
    return False


def metadata(raw):
    p = 0
    def vint():
        nonlocal p
        value = 0
        for n in range(10):
            b = raw[p]; p += 1
            value |= (b & 127) << (n * 7)
            if b < 128: return value
        raise ValueError()
    fields = [0] * 14
    while p < len(raw):
        tag = vint()
        if tag & 7 or not 1 <= tag >> 3 <= 14: raise ValueError()
        fields[(tag >> 3) - 1] = vint()
    return fields


def worker(row):
    global COMPLETE
    try:
        if WEB:
            row.pop('account_mac', None)
            row['captcha_id'] = '37ab6b750c3246af804132808408f398'
            row['app_key'] = 'b03cfcd909dbf05c25163cc8c7e7b6cf'
            row['device_id'] = str(uuid.uuid4())
            data = Path('/tmp/dh-web-acceptance')
            if not (data / 'admin-password.hash').is_file():
                raise ValueError()
            # Deployment material is transferred over stdin, never a config file.
            log = os.open('/tmp/dh-rust-web.log', os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
            try:
                child = subprocess.Popen([
                    str(ROOT / 'crates/dh-server/target/debug/dh-server'),
                    'serve-rust', str(data), str(ROOT / 'tauri3/dist-web')],
                    stdin=subprocess.PIPE, stdout=log, stderr=log,
                    env={**os.environ, 'DH_PROTOCOL_CONFIG': '/dev/stdin'}, start_new_session=True)
            finally:
                os.close(log)
            child.stdin.write(json.dumps(row).encode()); child.stdin.close()
            time.sleep(2)
            print('rust_web_started=' + str(child.poll() is None), flush=True)
            return
        if LOGIN:
            child = subprocess.run(['node', str(ROOT / 'tools/probe_rust_login.mjs')],
                                   input=json.dumps(row), text=True, timeout=1820)
            print('rust_login_probe_exit=' + str(child.returncode), flush=True)
            return
        snapshot = subprocess.run(['node','--input-type=module','-e',
            "import {readNimConfig} from './tools/probe_rust_nim.mjs'; process.stdout.write(JSON.stringify(await readNimConfig()));"],
            cwd=ROOT, capture_output=True, text=True, timeout=8)
        if snapshot.returncode != 0: raise ValueError()
        row['nim'] = json.loads(snapshot.stdout)
        child = subprocess.run([str(ROOT / 'crates/dh-protocol/target/debug/examples/business_probe')],
                               input=json.dumps(row), text=True, capture_output=True, timeout=65)
        # The Rust probe deliberately has no raw response/credential output.
        print(child.stdout, end='', flush=True)
        print(child.stderr, end='', flush=True)
        print('rust_business_probe_exit=' + str(child.returncode), flush=True)
    except Exception:
        print('rust_business_probe_execution_failed', flush=True)
    finally:
        row.clear()
        COMPLETE = True


def request_hit(frame, bp, internal):
    global WORKER
    if WORKER is not None: return False
    try:
        p = frame.GetThread().GetProcess()
        context = frame.FindRegister('x20').GetValueAsUnsigned()
        mac = MACS.pop(context, None)
        if mac is None and not LOGIN: return False
        request = frame.FindRegister('x19').GetValueAsUnsigned()
        headers = {}
        node = number(p, request + 0xe0)
        for _ in range(32):
            if not node: break
            error = lldb.SBError()
            line = p.ReadCStringFromMemory(number(p, node), 8192, error)
            if error.Fail(): raise ValueError()
            name, sep, value = line.partition(':')
            if sep: headers[name.lower()] = value.strip()
            node = number(p, node + 8)
        encoded = headers['x-request']
        fields = metadata(base64.urlsafe_b64decode(encoded + '=' * (-len(encoded) % 4)))
        if fields[3] == 0 and not LOGIN: return False
        address = read(p, number(p, request + 8), number(p, request)).decode()
        url = urlsplit(address)
        if url.scheme != 'https' or not url.hostname: raise ValueError()
        signer = number(p, BASE + 0x36fdc0)
        b64 = lambda value: base64.b64encode(value).decode()
        row = {'origin': url.scheme + '://' + url.netloc, 'headers': headers, 'metadata': fields,
               'signing_seed': b64(read(p, signer + 8, 32)),
               'body_key': b64(read(p, BASE + 0x34b760, 32)), 'account_mac': b64(mac) if mac else ''}
        if LOGIN:
            row['account_mac'] = ''
            row['headers'] = {k:v for k,v in headers.items() if k in ('x-version','x-device','content-type','accept')}
        row['server_key'] = b64(read(p, BASE + 0x36fdc8, 32))
        WORKER = threading.Thread(target=worker, args=(row,))
        WORKER.start()
        print('rust_business_probe_dispatched', flush=True)
    except Exception:
        print('rust_business_handoff_failed', flush=True)
    return False


def run(debugger, command, result, internal):
    global BASE, WORKER, COMPLETE, LOGIN, WEB
    if command.strip() not in ('', 'login', 'web'):
        raise ValueError('Expected login or web')
    WEB = command.strip() == 'web'
    LOGIN = command.strip() in ('login', 'web')
    WORKER = None; COMPLETE = False; MACS.clear()
    target = debugger.GetSelectedTarget(); process = target.GetProcess()
    previous = debugger.GetAsync(); ids = []
    try:
        path = Path(str(target.GetExecutable()))
        if hashlib.sha256(path.read_bytes()).hexdigest() != EXPECTED: raise ValueError('Unsupported sample')
        BASE = target.FindModule(target.GetExecutable()).GetObjectFileHeaderAddress().GetLoadAddress(target)
        for offset, expected, callback in [(0x1ee10, 'f80300aa', 'account_hit'), (0x1efcc, '886a41b9', 'request_hit')]:
            if read(process, BASE + offset, 4) != bytes.fromhex(expected): raise ValueError('Instruction mismatch')
            bp = target.BreakpointCreateByAddress(BASE + offset); ids.append(bp.GetID())
            bp.SetScriptCallbackFunction('probe_business_session.' + callback)
        debugger.SetAsync(True)
        if process.Continue().Fail(): raise ValueError('Cannot resume')
        print('business_handoff_ready', flush=True)
        query = subprocess.Popen(['cargo','test','--manifest-path','tauri3/src-tauri/Cargo.toml','--test','macos_real_protocol','verifies_redacted_business_reads','--','--ignored'],cwd=ROOT,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        start = time.monotonic()
        try:
            while time.monotonic() - start < 75 and not COMPLETE and not (LOGIN and WORKER): time.sleep(0.1)
        finally:
            if query.poll() is None: query.terminate()
            query.wait(timeout=5)
    finally:
        process.Stop()
        for ident in ids: target.BreakpointDelete(ident)
        print('business_handoff_detached=' + str(process.Detach().Success()), flush=True)
        debugger.SetAsync(previous); MACS.clear()
        if WORKER: WORKER.join(timeout=1830 if LOGIN else 70)


def __lldb_init_module(debugger, internal):
    debugger.HandleCommand('command script add -f probe_business_session.run dh-business-handoff')
