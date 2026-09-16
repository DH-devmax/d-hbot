// Passive protocol shape observer. Never prints endpoints, identities or values.
function shape(data) {
 let p=0;
 function byte(){if(p>=data.length)throw Error();return data[p++]}
 function vint(){let n=0;for(let i=0;i<5;i++){const b=byte();n+=(b&127)*2**(i*7);if(b<128)return n}throw Error()}
 function property(){const n=vint();if(n>128)throw Error();const ids=[];for(let i=0;i<n;i++){const id=vint(),len=vint();if(len>data.length-p)throw Error();p+=len;ids.push(id)}return ids}
 vint();const service=byte(),command=byte();byte();byte();const tag=byte();const code=tag&2?byte()+byte()*256:200;
 const out={service,command,code,body_bytes:data.length-p};
 if(service===2&&command===3&&code===200){try{out.login_fields=property();const n=vint();if(n>128)throw Error();out.port_count=n;for(let i=0;i<n;i++)property();out.push_fields=property();out.remaining=data.length-p}catch{out.body_parse_failed=true}}
 return out;
}
async function main(){
 const pages=await(await fetch('http://127.0.0.1:9222/json/list',{signal:AbortSignal.timeout(3000)})).json();
 const page=pages.find(p=>p.type==='page'&&new URL(p.url).pathname.endsWith('/dist/index.html'));
 if(!page)throw Error();
 const ws=new WebSocket(page.webSocketDebuggerUrl);
 const timer=setTimeout(()=>ws.close(),600000);
 ws.onopen=()=>ws.send(JSON.stringify({id:1,method:'Network.enable'}));
 ws.onmessage=e=>{try{const r=JSON.parse(e.data);if(r.id===1)console.log('nim_observer_ready');if(['Network.webSocketFrameReceived','Network.webSocketFrameSent'].includes(r.method)&&r.params.response.opcode===2){const bytes=Buffer.from(r.params.response.payloadData,'base64');const s=shape(bytes);if((s.service===2&&s.command===3)||(s.service===1&&s.command===2))console.log(JSON.stringify({direction:r.method.endsWith('Sent')?'sent':'received',...s}))}}catch{console.log('nim_shape_unclassified')}};
 ws.onclose=()=>clearTimeout(timer);
 ws.onerror=()=>{clearTimeout(timer);ws.close()};
 process.once('SIGTERM',()=>ws.close());process.once('SIGINT',()=>ws.close());
}
main().catch(()=>{console.error('nim_observer_unavailable');process.exitCode=1});
