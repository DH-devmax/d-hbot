// Research-only human-input bridge. Service requests are performed by Rust.
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { randomUUID } from 'node:crypto';

async function main(){
 const chunks=[];let length=0;
 for await(const chunk of process.stdin){length+=chunk.length;if(length>131072)throw Error();chunks.push(chunk)}
 const input=JSON.parse(Buffer.concat(chunks).toString());chunks.length=0;
 // Public application key from the pinned official desktop resource.
 input.app_key='b03cfcd909dbf05c25163cc8c7e7b6cf';
 input.device_id=randomUUID();
 const pages=await(await fetch('http://127.0.0.1:9222/json/list',{signal:AbortSignal.timeout(3000)})).json();
 const matches=pages.filter(p=>p.type==='page'&&new URL(p.url).protocol==='file:'&&new URL(p.url).pathname.endsWith('/dist/index.html'));
 if(matches.length!==1)throw Error();
 const url=new URL(matches[0].webSocketDebuggerUrl);
 if(url.protocol!=='ws:'||url.hostname!=='127.0.0.1'||url.port!=='9222')throw Error();
 const ws=new WebSocket(url);let sequence=0;const pending=new Map();let child;let captured=false;
 const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++sequence;const timer=setTimeout(()=>{pending.delete(id);reject(Error())},5000);pending.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params}))});
 const binding='dhRustLoginInput'+randomUUID().replaceAll('-','');
 const state='__'+binding;
 let done;const finished=new Promise(resolve=>{done=resolve});
 const stop=()=>{child?.kill('SIGTERM');done(1)};
 process.once('SIGINT',stop);process.once('SIGTERM',stop);
 ws.onmessage=async event=>{try{
  const message=JSON.parse(event.data);
  if(message.id){const p=pending.get(message.id);if(!p)return;pending.delete(message.id);clearTimeout(p.timer);message.error?p.reject(Error()):p.resolve(message.result);return}
  if(message.method!=='Runtime.bindingCalled'||message.params.name!==binding||captured)return;
  captured=true;input.login=JSON.parse(message.params.payload);
  console.log('rust_login_input_received; credentials_not_printed');
  child=spawn(resolve('crates/dh-protocol/target/debug/examples/login_probe'),[],{stdio:['pipe','pipe','pipe']});
  child.stdin.on('error',()=>{});child.stdin.end(JSON.stringify(input));delete input.login;
  child.stdout.on('data',data=>process.stdout.write(data));child.stderr.on('data',data=>process.stderr.write(data));
  child.once('error',()=>done(1));child.once('exit',code=>done(code===0?0:1));
 }catch{done(1)}};
 await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject});
 const timer=setTimeout(stop,1800000);
 try{
  await call('Runtime.enable');await call('Runtime.addBinding',{name:binding});
  const result=await call('Runtime.evaluate',{awaitPromise:true,returnByValue:true,expression:`(async()=>{
   const {C}=await import('./assets/zh-cn-0e3f1af2.js');
   if(typeof C.handleReq!=='function'||window[${JSON.stringify(state)}])throw Error();
   const original=C.handleReq;
   const entry={original,resolve:null};window[${JSON.stringify(state)}]=entry;
   entry.wrapper=function(path,params,...rest){
    if(typeof path==='string'&&path.trim()==='/v1/user/login'){
     if(entry.resolve)return Promise.resolve({error:true,code:-1,msg:'Rust verification is running'});
     return new Promise(resolve=>{entry.resolve=resolve;window[${JSON.stringify(binding)}](JSON.stringify(params));});
    }
    return original.call(this,path,params,...rest);
   };C.handleReq=entry.wrapper;return true;
  })()`});
  if(result.exceptionDetails||result.result?.value!==true)throw Error();
  console.log('rust_login_bridge_ready; human_validation_required=true');
  process.exitCode=await finished;
 }finally{
  clearTimeout(timer);child?.kill('SIGTERM');
  const message=process.exitCode===0?'Rust login test passed':captured?'Rust login test failed; see diagnostic result':'Rust login test timed out or stopped';
  await call('Runtime.evaluate',{awaitPromise:true,expression:`(async()=>{const {C}=await import('./assets/zh-cn-0e3f1af2.js');const entry=window[${JSON.stringify(state)}];if(entry){if(C.handleReq===entry.wrapper)C.handleReq=entry.original;entry.resolve?.({error:true,code:-1,msg:${JSON.stringify(message)}});delete window[${JSON.stringify(state)}];}})()`}).catch(()=>{});
  await call('Runtime.removeBinding',{name:binding}).catch(()=>{});ws.close();
  process.removeListener('SIGINT',stop);process.removeListener('SIGTERM',stop);
  console.log('rust_login_bridge_restored');
 }
}
main().catch(()=>{console.error('rust_login_bridge_failed; credentials_not_printed');process.exitCode=1});
