"""Research-only LLDB command. Emits counts, code offsets and fixed classifications.
Usage: lldb -p PID -o 'command script import tools/observe_native_socket.py'
Then: dh-count
Only attach to the reviewed arm64 mclient. Query is read-only and bounded.
requested_bytes is the syscall input size, NOT bytes transferred (recv capacity).
"""
import lldb, time, json, subprocess, hashlib, shlex
from pathlib import Path
from native_http_shape import MAX_HEADER_BYTES, request_shape, h2_shape
EXPECTED = '815b95a3357b2b22492a5a9fc51dc645e019023459155027a656d1902beb4a70'
ROOT = Path(__file__).resolve().parent.parent
counts={}
def header_submit_hit(frame,bp,internal):
    row=counts.setdefault('header_submit_candidate',{'calls':0})
    row['calls']+=1
    count=frame.FindRegister('x4').GetValueAsUnsigned()
    if not 0<count<=64:
        row['invalid_count']=row.get('invalid_count',0)+1
        return False
    process=frame.GetThread().GetProcess()
    address=frame.FindRegister('x3').GetValueAsUnsigned()
    known={b':method',b':path',b':scheme',b':authority',b'content-type',
           b'content-length',b'authorization',b'cookie',b'user-agent',b'accept',
           # Fixed names independently recovered from this hash-pinned sample.
           b'x-context',b'x-version',b'x-trace-id',b'x-device',b'x-id',
           b'x-token',b'x-jwt',b'x-request',b'x-seed',b'x-hash'}
    labels=[]
    for i in range(count):
        error=lldb.SBError()
        entry=process.ReadMemory(address+i*40,32,error)
        if error.Fail() or len(entry)!=32:
            labels.append('unreadable'); break
        pointer=int.from_bytes(entry[:8],'little')
        length=int.from_bytes(entry[16:24],'little')
        del entry
        if not 0<length<=64:
            labels.append('other'); continue
        name=process.ReadMemory(pointer,length,error)
        label=name.decode('ascii') if error.Success() and name in known else 'other'
        del name
        labels.append(label)
    shape=json.dumps(sorted(labels))
    shapes=row.setdefault('name_shapes',{})
    shapes[shape]=shapes.get(shape,0)+1
    return False

def classify_prefix(prefix):
    for method in (b'GET ',b'POST ',b'PUT ',b'DELETE ',b'PATCH ',b'HEAD ',b'OPTIONS '):
        if prefix.startswith(method):
            return 'http_request'
    if prefix.startswith(b'HTTP/1.'):
        return 'http_response'
    return 'unclassified'

def tls_boundary_hit(frame,bp,internal):
    # Inspect at most eight bytes in memory; never emit or persist these bytes.
    row=counts.setdefault('tls_dispatch_candidate',{'calls':0,'requested_bytes':0})
    row['calls']+=1
    size=frame.FindRegister('x2').GetValueAsUnsigned()
    if size < 16777216:
        row['requested_bytes']+=size
    else:
        row['oversize_calls']=row.get('oversize_calls',0)+1
    classification='unreadable'
    if 0 < size < 16777216:
        error=lldb.SBError()
        prefix=frame.GetThread().GetProcess().ReadMemory(
            frame.FindRegister('x1').GetValueAsUnsigned(),min(size,8),error)
        if error.Success():
            classification=classify_prefix(prefix)
        del prefix
        if classification=='http_request':
            header=frame.GetThread().GetProcess().ReadMemory(
                frame.FindRegister('x1').GetValueAsUnsigned(),min(size,MAX_HEADER_BYTES),error)
            if error.Success():
                shape=json.dumps(request_shape(header),sort_keys=True)
                shapes=row.setdefault('http_shapes',{})
                shapes[shape]=shapes.get(shape,0)+1
            del header
        elif classification=='unclassified' and size<=MAX_HEADER_BYTES:
            payload=frame.GetThread().GetProcess().ReadMemory(
                frame.FindRegister('x1').GetValueAsUnsigned(),size,error)
            if error.Success():
                shape=json.dumps(h2_shape(payload),sort_keys=True)
                shapes=row.setdefault('binary_shapes',{})
                shapes[shape]=shapes.get(shape,0)+1
            del payload
    classes=row.setdefault('prefix_classes',{})
    classes[classification]=classes.get(classification,0)+1
    target=frame.GetThread().GetProcess().GetTarget()
    destination=target.ResolveLoadAddress(frame.FindRegister('x8').GetValueAsUnsigned())
    module=destination.GetModule()
    label='other_module'
    if module.IsValid() and module.GetFileSpec().GetFilename()=='mclient':
        label='mclient+0x%x' % (destination.GetFileAddress()-module.GetObjectFileHeaderAddress().GetFileAddress())
    targets=row.setdefault('targets',{})
    targets[label]=targets.get(label,0)+1
    return False

def hit(frame,bp,internal):
    name=frame.GetFunctionName() or 'unknown'
    if name not in ('send','sendto','sendmsg','recv','recvfrom','recvmsg','write','writev'): name='unknown'
    row=counts.setdefault(name,{'calls':0,'requested_bytes':0})
    row['calls']+=1
    # File descriptors are process-local integers, not protocol identities.
    descriptor=frame.FindRegister('w0').GetValueAsUnsigned()
    if descriptor < 65536:
        descriptors=row.setdefault('descriptors',{})
        descriptors[str(descriptor)]=descriptors.get(str(descriptor),0)+1
    caller_frame=frame.GetThread().GetFrameAtIndex(1)
    module=caller_frame.GetModule()
    caller=module.GetFileSpec().GetFilename()
    if caller == 'mclient':
        address=caller_frame.GetPCAddress().GetFileAddress()
        base=module.GetObjectFileHeaderAddress().GetFileAddress()
        caller='mclient+0x%x' % (address-base)
    else:
        caller='other_module'
    row.setdefault('callers',{})[caller or 'unknown']=row.setdefault('callers',{}).get(caller or 'unknown',0)+1
    if name == 'send':
        chain=[]
        thread=frame.GetThread()
        for depth in range(1,min(thread.GetNumFrames(),9)):
            f=thread.GetFrameAtIndex(depth)
            m=f.GetModule()
            if m.GetFileSpec().GetFilename() == 'mclient':
                address=f.GetPCAddress().GetFileAddress()
                base=m.GetObjectFileHeaderAddress().GetFileAddress()
                chain.append(hex(address-base))
            else:
                chain.append('other_module')
        key=','.join(chain)
        row.setdefault('stacks',{})[key]=row.setdefault('stacks',{}).get(key,0)+1
    if name in ('send','sendto','recv','recvfrom','write'):
        size=frame.FindRegister('x2').GetValueAsUnsigned()
        if size<16777216: row['requested_bytes']+=size
    return False
def run(debugger,command,result,internal):
    args=shlex.split(command)
    watch_seconds=None
    stop_path=None
    if args:
        if len(args)!=3 or args[0]!='watch':
            raise ValueError('Usage: dh-count [watch SECONDS ABSOLUTE_STOP_FILE]')
        watch_seconds=int(args[1])
        stop_path=Path(args[2])
        if not 1 <= watch_seconds <= 600 or not stop_path.is_absolute() or stop_path.exists():
            raise ValueError('Invalid duration or stop file; use a fresh absolute path')
    target=debugger.GetSelectedTarget(); process=target.GetProcess()
    path=Path(str(target.GetExecutable()))
    if path.name != 'mclient' or hashlib.sha256(path.read_bytes()).hexdigest() != EXPECTED:
        process.Detach()
        raise RuntimeError('Unreviewed target; detached without installing probes')
    if not target.GetTriple().startswith('arm64'):
        process.Detach()
        raise RuntimeError('Unsupported architecture')
    counts.clear()
    ids=[]
    query=None
    previous_async=debugger.GetAsync()
    try:
        module=target.FindModule(target.GetExecutable())
        address=module.GetObjectFileHeaderAddress().GetLoadAddress(target)+0x83e74
        error=lldb.SBError()
        if process.ReadMemory(address,4,error)!=bytes.fromhex('00013fd6') or error.Fail():
            raise RuntimeError('TLS dispatch instruction mismatch')
        bp=target.BreakpointCreateByAddress(address)
        ids.append(bp.GetID())
        bp.SetScriptCallbackFunction('observe_native_socket.tls_boundary_hit')
        header_address=module.GetObjectFileHeaderAddress().GetLoadAddress(target)+0x10801c
        if process.ReadMemory(header_address,4,error)!=bytes.fromhex('ff8302d1') or error.Fail():
            raise RuntimeError('Header submit instruction mismatch')
        bp=target.BreakpointCreateByAddress(header_address)
        ids.append(bp.GetID())
        bp.SetScriptCallbackFunction('observe_native_socket.header_submit_hit')
        for name in ('send','sendto','sendmsg','recv','recvfrom','recvmsg','write','writev'):
            bp=target.BreakpointCreateByName(name); bp.SetScriptCallbackFunction('observe_native_socket.hit'); ids.append(bp.GetID())
        locations=sum(target.FindBreakpointByID(i).GetNumLocations() for i in ids)
        if locations==0:
            raise RuntimeError('No resolved probe locations')
        debugger.SetAsync(True)
        if process.Continue().Fail():
            raise RuntimeError('Target failed to resume')
        print(json.dumps({'kind':'probe_ready','locations':locations,'at':time.time()}),flush=True)
        start=time.monotonic()
        if watch_seconds is not None:
            while time.monotonic()-start < watch_seconds and not stop_path.exists():
                if process.GetState() in (lldb.eStateExited,lldb.eStateDetached,lldb.eStateInvalid):
                    break
                time.sleep(0.1)
            return
        query=subprocess.Popen(['cargo','test','--manifest-path','tauri3/src-tauri/Cargo.toml','--test','macos_real_protocol','verifies_redacted_business_reads','--','--ignored'],cwd=ROOT,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        while query.poll() is None and time.monotonic()-start < 50:
            time.sleep(0.1)
        if query.poll() is None:
            query.terminate(); query.wait(timeout=5)
            print('query_timeout')
        else:
            print('query_exit='+str(query.returncode))
        print('observed_seconds='+str(round(time.monotonic()-start,2)))
    finally:
        if query is not None and query.poll() is None:
            query.terminate()
            try: query.wait(timeout=3)
            except subprocess.TimeoutExpired:
                query.kill(); query.wait()
        process.Stop()
        for i in ids: target.BreakpointDelete(i)
        error=process.Detach()
        debugger.SetAsync(previous_async)
        print(json.dumps({'counts':counts,'detached':error.Success(),'at':time.time()}),flush=True)
def __lldb_init_module(debugger,internal):
    debugger.HandleCommand('command script add -f observe_native_socket.run dh-count')
