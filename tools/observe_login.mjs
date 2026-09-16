// Research-only passive observer. Never persists URL, headers, IDs or payloads.
import { pathToFileURL } from 'node:url';
export function redact(event) {
  const p = event.params || {};
  switch (event.method) {
    case 'Network.requestWillBeSent': return { kind: 'request', method: ['GET','POST','PUT','DELETE','OPTIONS','HEAD','PATCH'].includes(p.request?.method) ? p.request.method : 'other' };
    case 'Network.responseReceived': return { kind: 'response', status: Number.isInteger(p.response?.status) ? p.response.status : 0 };
    case 'Network.webSocketCreated': return { kind: 'socket_created' };
    case 'Network.webSocketWillSendHandshakeRequest': return { kind: 'handshake_request' };
    case 'Network.webSocketHandshakeResponseReceived': return { kind: 'handshake_response', status: Number.isInteger(p.response?.status) ? p.response.status : 0 };
    case 'Network.webSocketClosed': return { kind: 'socket_closed' };
    case 'Network.webSocketFrameSent':
    case 'Network.webSocketFrameReceived': return { kind: event.method.endsWith('Sent') ? 'frame_sent' : 'frame_received', opcode: Number.isInteger(p.response?.opcode) ? p.response.opcode : -1, bytes: typeof p.response?.payloadData === 'string' ? Buffer.byteLength(p.response.payloadData, p.response.opcode === 2 ? 'base64' : 'utf8') : 0 };
    default: return null;
  }
}
async function main() {
  const pages = await (await fetch('http://127.0.0.1:9222/json/list', { signal: AbortSignal.timeout(3000) })).json();
  const matches = pages.filter(p => { try { const u = new URL(p.url); return p.type === 'page' && u.protocol === 'file:' && u.pathname.endsWith('/dist/index.html'); } catch { return false; } });
  if (matches.length !== 1) throw Error();
  const u = new URL(matches[0].webSocketDebuggerUrl);
  if (u.protocol !== 'ws:' || u.hostname !== '127.0.0.1' || u.port !== '9222' || !u.pathname.startsWith('/devtools/page/')) throw Error();
  const ws = new WebSocket(u);
  let ready = false, count = 0, stopped = false;
  const started = performance.now();
  const emit = x => process.stdout.write(JSON.stringify({at:new Date().toISOString(),elapsed_ms:Math.round(performance.now()-started),...x}) + '\n');
  const stop = () => { if(stopped) return; stopped=true; clearTimeout(timer); clearTimeout(startup); ws.close(); emit({kind:'stopped',events:count}); };
  const timer = setTimeout(stop, 10 * 60 * 1000);
  const startup = setTimeout(() => { if (!ready) { emit({kind:'setup_timeout'}); stop(); } }, 5000);
  process.once('SIGINT', stop); process.once('SIGTERM', stop);
  ws.onopen = () => ws.send(JSON.stringify({id:1,method:'Network.enable',params:{maxTotalBufferSize:0,maxResourceBufferSize:0,maxPostDataSize:0}}));
  ws.onmessage = e => { try { const event = JSON.parse(e.data); if (event.id === 1) { if (event.error) return stop(); ready=true; clearTimeout(startup); emit({kind:'ready',scope:'renderer_network_only',native_requests:'not_observed',duration_seconds:600}); } const safe=redact(event); if(safe && ready) { count++; emit(safe); } } catch {} };
  ws.onerror = () => { emit({kind:'connection_error'}); stop(); };
  ws.onclose = stop;
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main().catch(() => { console.error('observer_setup_failed'); process.exitCode=1; });
