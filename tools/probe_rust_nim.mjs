// Research-only handoff. Production Rust never invokes CDP or this script.
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const expression = `(()=>{
 const p=window.nim?.protocol;
 if(!p?.hasLogin||typeof p.assembleIMLogin!=='function') throw Error('session_required');
 const clone=Object.create(p);
 clone.genSessionKey=()=>crypto.randomUUID();
 const fields=p.assembleIMLogin.call(clone);
 const endpoint=p.socket?.socket?.url||p.socket?.socketUrl||p.url;
 if(typeof endpoint!=='string'||!endpoint.startsWith('wss://')) throw Error('endpoint_unavailable');
 return {endpoint,fields:Object.fromEntries(Object.entries(fields).filter(([,v])=>v!==undefined&&v!==null).map(([k,v])=>[k,typeof v==='string'?v:JSON.stringify(v)]))};
})()`;
export async function readNimConfig() {
 const pages=await(await fetch('http://127.0.0.1:9222/json/list',{signal:AbortSignal.timeout(3000)})).json();
 const matches=pages.filter(p=>p.type==='page'&&new URL(p.url).protocol==='file:'&&new URL(p.url).pathname.endsWith('/dist/index.html'));
 if(matches.length!==1) throw Error();
 const u=new URL(matches[0].webSocketDebuggerUrl);
 if(u.protocol!=='ws:'||u.hostname!=='127.0.0.1'||u.port!=='9222') throw Error();
 const payload=await new Promise((resolve,reject)=>{
  const ws=new WebSocket(u);
  const timer=setTimeout(()=>{ws.close();reject(Error())},5000);
  ws.onopen=()=>ws.send(JSON.stringify({id:1,method:'Runtime.evaluate',params:{expression,returnByValue:true}}));
  ws.onerror=()=>{clearTimeout(timer);reject(Error())};
  ws.onmessage=e=>{const r=JSON.parse(e.data);if(r.id!==1)return;clearTimeout(timer);ws.close();if(r.error||r.result?.exceptionDetails||!r.result?.result?.value)return reject(Error());resolve(r.result.result.value)};
 });
 return payload;
}
async function main() {
 const payload=await readNimConfig();
 const child=spawn(resolve('crates/dh-protocol/target/debug/examples/nim_probe'),[],{stdio:['pipe','inherit','inherit']});
 child.stdin.on('error',()=>{});
 child.stdin.end(JSON.stringify(payload));
 payload.fields.token='';
 const timer=setTimeout(()=>child.kill('SIGTERM'),40000);
 const code=await new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',code=>resolve(code))}).finally(()=>clearTimeout(timer));
 process.exitCode=code===0?0:1;
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href) main().catch(()=>{console.error('research_handoff_failed; credentials_not_printed');process.exitCode=1});
