// Run isolated header methods from the reviewed local SDK against synthetic bytes.
// No module initialization, credentials, filesystem inside VM or network access.
import { readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { runInNewContext } from 'node:vm'
import assert from 'node:assert/strict'
const archive=readFileSync(process.argv[2] || '/Applications/旺商聊.app/Contents/Resources/app.asar')
const tree=JSON.parse(archive.subarray(16,16+archive.readUInt32LE(12)))
const node=tree.files.dist.files.assets.files['index-bcfa60a0.js']
const start=8+archive.readUInt32LE(4)+Number(node.offset)
const bytes=archive.subarray(start,start+node.size)
assert.equal(createHash('sha256').update(bytes).digest('hex'),'2deaa52d04248c03bf9dd5fc72066644afcd438c3e527c5a1dd5b68c54172020','Unreviewed SDK')
const source=bytes.toString()
function method(name){const prefix=`key:"${name}",value:`;const begin=source.indexOf(prefix);assert.ok(begin>=0);const end=source.indexOf('},{key:',begin);assert.ok(end>begin);return source.slice(begin+prefix.length,end)}
const methods=Object.fromEntries(['popByte','popVarInt','popShort','_unmarshalHeader','hasRescode'].map(n=>[n,method(n)]))
for(const input of [[172,2,4,10,52,18,2,200,0],[5,1,2,0,0,0]]){
 const result=runInNewContext(`const b=Uint8Array.from(input);const unpack={view:new DataView(b.buffer),offset:0,popByte:(${methods.popByte}),popVarInt:(${methods.popVarInt}),popShort:(${methods.popShort})};const parser={unpack,hasRescode:(${methods.hasRescode}),parse:(${methods._unmarshalHeader})};JSON.stringify({...parser.parse(),consumed:unpack.offset})`,{input},{timeout:1000})
 const actual=JSON.parse(result)
 assert.equal(actual.packetLength,input.length===9?300:5)
 assert.equal(actual.serialId,input.length===9?4660:0)
 assert.equal(actual.resCode,200)
 assert.equal(actual.consumed,input.length)
}
console.log('SDK header oracle: 2 synthetic vectors passed; no SDK initialization')
const marshal = method('marshalHeader')
for (const [serviceId,commandId,serialId] of [[1,2,0],[4,10,4660]]) {
 const result=runInNewContext(`const out=[]; const encoder={packetLength:0,serviceId,commandId,serialId,tag:0,pack:{putVarInt(v){if(v!==0)throw Error();out.push(0)},putByte(v){out.push(v)},putShort(v){out.push(v&255,v>>>8)}},write:(${marshal})};encoder.write();JSON.stringify(out)`,{serviceId,commandId,serialId},{timeout:1000})
 assert.deepEqual(JSON.parse(result),[0,serviceId,commandId,serialId&255,serialId>>>8,0])
}
console.log('SDK outgoing header oracle: 2 synthetic vectors passed')
// Execute the recovered property method; helpers model its imported JS utilities.
const propertyMethod=method('marshalProperty')
const propertyResult=runInNewContext(`
const out=[];
const $=Object.keys,z=()=>Array.prototype.filter,ee=()=>Array.prototype.forEach;
const oe=v=>v==null,te=Array.isArray,ie=JSON.stringify;
function put(v){while(v>=128){out.push((v&127)|128);v>>>=7}out.push(v)}
const encoder={pack:{putVarInt:put,putString(v){const b=Array.from(v,c=>c.charCodeAt(0));put(b.length);out.push(...b)}},write:(${propertyMethod})};
encoder.write({19:'demo',1000:'synthetic'});JSON.stringify(out)
`,{},{timeout:1000})
assert.deepEqual(JSON.parse(propertyResult),[2,19,4,100,101,109,111,232,7,9,115,121,110,116,104,101,116,105,99])
console.log('SDK Property oracle: synthetic ASCII login-field vector passed; transport authentication not tested')
// Observe actual assembler branches using synthetic environment and credentials.
const assembleStart=source.indexOf('ee.assembleIMLogin=function()')
assert.ok(assembleStart>=0)
const assembleEnd=source.indexOf(',ee.notifyLogin=',assembleStart)
assert.ok(assembleEnd>assembleStart)
const assemble=source.slice(assembleStart+'ee.assembleIMLogin='.length,assembleEnd)
for(const compat of [false,true]) {
 const result=JSON.parse(runInNewContext(`
 const z=()=>String.prototype.slice;
 const ce={info:{sdkVersion:'92114',sdkHumanVersion:'9.21.14',protocolVersion:1},CLIENTTYPE:16,isRN:false,isBrowser:true};
 const oe={os:{toString(){return 'SyntheticOS'}},name:'SyntheticBrowser',version:'1'};
 const se={deviceId:'synthetic-device'},navigator={userAgent:'x'.repeat(350)};
 const ue=()=>7,ae=()=>8;
 const client={options:{account:'synthetic-account',token:'synthetic-token',appKey:'synthetic-app',privateConf:{loginSDKTypeParamCompat:compat},customClientType:'42',authType:'2',loginExt:'synthetic-extension'},autoconnect:true,genSessionKey:()=> 'synthetic-session',assemble:(${assemble})};
 JSON.stringify(client.assemble())`,{compat},{timeout:1000}))
 assert.equal(result.appLogin,0)
 assert.equal(result.session,'synthetic-session')
 assert.equal(result.deviceId,'synthetic-device')
 assert.equal(result.account,'synthetic-account')
 assert.equal(result.token,'synthetic-token')
 assert.equal(result.customClientType,42)
 assert.equal(result.authType,2)
 assert.equal(result.loginExt,'synthetic-extension')
 assert.equal(result.userAgent,compat?'Native/9.21.14':'x'.repeat(299))
 assert.equal(result.sdkType,compat?0:8)
 assert.equal(result.libEnv,compat?undefined:7)
}
console.log('SDK login assembler: 2 synthetic compatibility branches passed; no live authentication')
// Pinned SDK evidence for team send and real-time notification encapsulation.
assert.ok(source.includes('sendTeamMsg:{sid:re.team.id,cid:re.team.sendTeamMsg,params:[{type:"Property",name:"msg"}]}'))
assert.ok(source.includes('team:{id:8,createTeam:1,sendTeamMsg:2'))
const notify = method('unmarshalHeader')
const wrapped = JSON.parse(runInNewContext(`
 const z=()=>Array.prototype.includes;
 const headers=[{packetLength:0,serviceId:4,commandId:1,serialId:0,tag:0,resCode:200},{packetLength:0,serviceId:8,commandId:3,serialId:0,tag:0,resCode:200}];
 const parser={_unmarshalHeader(){return headers.shift()},unmarshalLong(){return '123456789'},parse:(${notify})};
 parser.parse();JSON.stringify({msgId:parser.msgId,inner:parser.innerHeader})`,{},{timeout:1000}))
assert.equal(wrapped.msgId,'123456789')
assert.equal(wrapped.inner.serviceId,8)
assert.equal(wrapped.inner.commandId,3)
console.log('SDK team route and nested notify oracle passed')
