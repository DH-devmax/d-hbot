import {test} from 'node:test';
import assert from 'node:assert/strict';
import {redact} from './observe_login.mjs';
test('handshake metadata excludes all credential headers',()=> {
 const params={request:{headers:{Cookie:'SECRET'}},response:{status:101,headers:{Authorization:'SECRET'}}};
 assert.deepEqual(redact({method:'Network.webSocketWillSendHandshakeRequest',params}),{kind:'handshake_request'});
 assert.deepEqual(redact({method:'Network.webSocketHandshakeResponseReceived',params}),{kind:'handshake_response',status:101});
 assert.deepEqual(redact({method:'Network.webSocketClosed',params}),{kind:'socket_closed'});
});
test('drops credentials, URLs, identifiers and arbitrary event fields', () => {
 const secret='SENSITIVE_SENTINEL';
 for(const method of ['Network.requestWillBeSent','Network.responseReceived','Network.webSocketCreated','Network.webSocketFrameSent','Network.webSocketFrameReceived','Runtime.consoleAPICalled']) {
 const out=redact({method,params:{requestId:secret,request:{method:'POST',url:secret,headers:{Cookie:secret},postData:secret},response:{status:200,opcode:1,payloadData:secret,headers:{Authorization:secret}}}});
 assert.ok(!JSON.stringify(out).includes(secret));
 }
});
test('binary frames count decoded bytes without emitting content',()=> {
 assert.deepEqual(redact({method:'Network.webSocketFrameReceived',params:{response:{opcode:2,payloadData:'AQID'}}}),{kind:'frame_received',opcode:2,bytes:3});
});
