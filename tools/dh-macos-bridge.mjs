import http from "node:http";

const listenHost = process.env.DH_BRIDGE_HOST || "127.0.0.1";
const listenPort = Number(process.env.DH_BRIDGE_PORT || 51235);
const devtoolsURL = process.env.DH_DEVTOOLS_URL || "http://127.0.0.1:9222";
const maxBodySize = 8 * 1024 * 1024;

class CDPClient {
  constructor() {
    this.socket = null;
    this.sequence = 0;
    this.pending = new Map();
  }

  async connect() {
    if (this.socket?.readyState === WebSocket.OPEN) return;
    const response = await fetch(`${devtoolsURL}/json/list`);
    if (!response.ok) throw new Error(`DevTools HTTP ${response.status}`);
    const pages = await response.json();
    const page = pages.find((item) => item.type === "page" && item.webSocketDebuggerUrl);
    if (!page) throw new Error("旺商聊 DevTools 页面未就绪");

    const socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", reject, { once: true });
    });
    socket.addEventListener("message", (event) => this.onMessage(event));
    socket.addEventListener("close", () => this.onClose());
    this.socket = socket;
    await this.call("Runtime.enable");
  }

  onMessage(event) {
    const message = JSON.parse(event.data);
    const waiter = this.pending.get(message.id);
    if (!waiter) return;
    this.pending.delete(message.id);
    if (message.error) waiter.reject(new Error(message.error.message));
    else waiter.resolve(message.result);
  }

  onClose() {
    this.socket = null;
    for (const waiter of this.pending.values()) waiter.reject(new Error("DevTools 连接已关闭"));
    this.pending.clear();
  }

  async call(method, params = {}) {
    await this.connect();
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate(expression) {
    const result = await this.call("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.text || "Electron 执行异常");
    }
    return result.result?.value;
  }
}

const cdp = new CDPClient();

const enumValues = {
  msgDevice: { System: 0, Desktop: 1, Ios: 2, Android: 3 },
  msgSession: {
    MSG_KIND_SYSTEM: 0,
    MSG_KIND_P2P: 1,
    MSG_KIND_GROUP: 2,
    MSG_KIND_GROUP_SYSTEM: 3,
    MSG_KIND_NOTIFICATION: 4,
    MSG_KIND_GROUP_NOTIFICATION: 5,
    MSG_BATCH_ATTACH__NOTIFICATION: 6,
    MSG_BROADCAST_SYSTEM: 7,
  },
  accountType: {
    ACCOUNT_MEMBER: 0,
    ACCOUNT_CUSTOMER_SERVICE: 1,
    ACCOUNT_FINANCE_SERVICE: 2,
    ACCOUNT_MERCHANT: 3,
    ACCOUNT_THIRD_SERVICE: 4,
    ACCOUNT_SYSTEM: 5,
    ACCOUNT_INNER_USER: 6,
  },
  msgFormat: {
    MSG_TEXT: 0,
    MSG_IMG: 1,
    MSG_VOICE: 2,
    MSG_VIDEO: 3,
    MSG_GEO: 4,
    MSG_FILE: 6,
    MSG_GROUP_NOTIFICATION: 7,
    MSG_GROUP_NOTICE: 8,
    MSG_FORWARD: 9,
    MSG_RED_ENVELOPE: 10,
    MSG_TRANSFER: 11,
    MSG_PLUGINS: 12,
    MSG_FRIEND_CARD: 13,
  },
  msgRole: { MSG_MINE: 0, MSG_MASTER: 1, MSG_ADMIN: 2, MSG_MEMBER: 3, MSG_SYSTEM: 4 },
  msgRingtone: { MSG_RINGTONE_NONE: 0 },
  appoint: { MSG_APPOINT_NONE: 0, MSG_APPOINT_QUOTE: 1, MSG_APPOINT_FORWARD: 2 },
};

function rendererMessage(message) {
  const normalized = structuredClone(message);
  for (const [field, values] of Object.entries(enumValues)) {
    if (typeof normalized[field] === "string" && normalized[field] in values) {
      normalized[field] = values[normalized[field]];
    }
  }
  const timestamp = normalized.created_at ?? normalized.createdAt;
  if (typeof timestamp === "string") {
    const milliseconds = Date.parse(timestamp);
    if (!Number.isFinite(milliseconds)) throw new Error("created_at 时间格式错误");
    normalized.createdAt = {
      seconds: Math.floor(milliseconds / 1000),
      nanos: Math.floor(milliseconds % 1000) * 1_000_000,
    };
  }
  delete normalized.created_at;
  return normalized;
}

function ipcExpression(type, route, payload) {
  const request = JSON.stringify({ type, route, payload });
  return `(async()=>{
    const input=${request};
    const ipc=require("electron").ipcRenderer;
    return new Promise((resolve)=>{
      const channel="dh-bridge-"+Date.now()+"-"+Math.random();
      const timer=setTimeout(()=>resolve({transportCode:504,errno:1,error:"IPC timeout"}),15000);
      ipc.once(channel,(event,value)=>{
        clearTimeout(timer);
        if(input.type==="decode"){
          let bytes=[];
          if(value instanceof ArrayBuffer) bytes=Array.from(new Uint8Array(value));
          else if(ArrayBuffer.isView(value)) bytes=Array.from(new Uint8Array(value.buffer,value.byteOffset,value.byteLength));
          resolve({transportCode:200,errno:0,binary:bytes});
          return;
        }
        resolve({
          transportCode:value&&value.code,
          errno:value&&value.errno,
          response:value&&value.response,
          error:value&&value.message
        });
      });
      if(input.type==="request"){
        ipc.send("xclient",{
          type:"request",requestId:channel,url:input.route,
          excuteType:input.route==="/v1/user/login"?2:0,
          params:JSON.stringify(input.payload??{}),key:channel
        });
      }else if(input.type==="encode"){
        ipc.send("xclient",{type:"encode",params:JSON.stringify(input.payload),key:channel});
      }else{
        ipc.send("xclient",{type:"decode",params:JSON.stringify(input.payload),key:channel});
      }
    });
  })()`;
}

function nimSendExpression(scene, target, content, localOnly = false) {
  const request = JSON.stringify({ scene, target: String(target), content, localOnly });
  return `(async()=>{
    const input=${request};
    return new Promise((resolve)=>{
      if(!window.nim){resolve({ok:false,errorCode:"NIM_NOT_READY",errorMessage:"NIM 未就绪"});return;}
      const timer=setTimeout(()=>resolve({ok:false,errorCode:"TIMEOUT",errorMessage:"NIM send timeout"}),15000);
      window.nim.sendCustomMsg({
        scene:input.scene,to:input.target,content:input.content,isLocal:input.localOnly,
        done:(error,message)=>{
          clearTimeout(timer);
          resolve({
            ok:!error,
            errorCode:error&&(error.code||error.name),
            errorMessage:error&&(error.message||String(error)),
            idClient:message&&message.idClient,
            idServer:message&&message.idServer,
            status:message&&message.status,
            flow:message&&message.flow,
            scene:message&&message.scene,
            to:message&&message.to
          });
        }
      });
    });
  })()`;
}

async function encodeMessage(message) {
  const result = await cdp.evaluate(ipcExpression("encode", "/v1/plugins/encode-msg", rendererMessage(message)));
  if (![0, 200].includes(result?.transportCode) || typeof result.response !== "string") {
    throw new Error(result?.error || `encode 失败 (${result?.transportCode ?? "unknown"}/${result?.errno ?? 0})`);
  }
  return result.response;
}

async function resolveGroupTarget(groupID) {
  const result = await cdp.evaluate(ipcExpression("request", "/v1/group/get-group-list", {}));
  if (result?.transportCode !== 200 || typeof result.response !== "string") {
    throw new Error("群列表读取失败");
  }
  const envelope = JSON.parse(result.response);
  if (envelope.code !== 0) throw new Error(envelope.msg || "群列表读取失败");
  const groups = [...(envelope.data?.owner || []), ...(envelope.data?.member || [])];
  const group = groups.find((item) => Number(item.groupId) === Number(groupID));
  if (!group?.groupCloudId) throw new Error("群 ID 未映射到 NIM 群");
  return String(group.groupCloudId);
}

async function readJSON(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > maxBodySize) throw new Error("请求体超过 8 MiB");
    chunks.push(chunk);
  }
  if (size === 0) return {};
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function sendJSON(response, status, value) {
  const body = JSON.stringify(value);
  response.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": Buffer.byteLength(body),
    "Access-Control-Allow-Origin": "*",
  });
  response.end(body);
}

async function handle(request, response) {
  const url = new URL(request.url, `http://${request.headers.host || listenHost}`);
  if (request.method === "OPTIONS") {
    response.writeHead(204, {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Headers": "Content-Type,X-Token,X-Group-Token,X-jwt,X-id",
      "Access-Control-Allow-Methods": "GET,POST,OPTIONS",
    });
    response.end();
    return;
  }
  if (url.pathname === "/ping") {
    const page = await cdp.evaluate("({title:document.title,route:location.hash,ready:document.readyState})");
    sendJSON(response, 200, { code: 0, errno: 0, msg: "OK", data: { bridge: "macOS", ...page } });
    return;
  }

  const payload = await readJSON(request);
  let result;
  if (url.pathname === "/v1/plugins/encode-msg") {
    if (!payload?.msg) throw new Error("encode-msg 缺少 msg");
    const content = await encodeMessage(payload.msg);
    sendJSON(response, 200, { code: 0, errno: 0, msg: "OK", data: { content } });
    return;
  }
  if (url.pathname === "/v1/plugins/send-msg") {
    if (!payload?.msg) throw new Error("send-msg 缺少 msg");
    const session = payload.msg.msgSession;
    const scene = payload.scene || (session === "MSG_KIND_GROUP" || session === 2 ? "team" : "p2p");
    let target = payload.nimTarget;
    if (!target && scene === "team") target = await resolveGroupTarget(payload.msg.to?.id);
    if (!target) throw new Error("send-msg 缺少 NIM 目标");
    const content = await encodeMessage(payload.msg);
    const delivery = await cdp.evaluate(nimSendExpression(scene, target, content, Boolean(payload.localOnly)));
    if (!delivery?.ok) {
      sendJSON(response, 502, { code: 502, errno: 1, msg: delivery?.errorMessage || "NIM 投递失败", data: { errorCode: delivery?.errorCode } });
      return;
    }
    sendJSON(response, 200, {
      code: 0,
      errno: 0,
      msg: "OK",
      data: {
        delivered: true,
        idClient: delivery.idClient,
        idServer: delivery.idServer,
        status: delivery.status,
        flow: delivery.flow,
        scene: delivery.scene,
      },
    });
    return;
  }
  if (url.pathname === "/v1/plugins/decode-msg") {
    if (typeof payload?.msg !== "string") throw new Error("decode-msg 缺少 msg");
    result = await cdp.evaluate(ipcExpression("decode", url.pathname, payload.msg));
    sendJSON(response, 200, { code: 0, errno: 0, msg: "OK", data: { bytes: result?.binary || [] } });
    return;
  }

  result = await cdp.evaluate(ipcExpression("request", url.pathname, payload));
  if (result?.transportCode !== 200 || typeof result.response !== "string") {
    sendJSON(response, 502, { code: result?.transportCode || 502, errno: result?.errno || 1, msg: result?.error || "xclient 请求失败" });
    return;
  }
  response.writeHead(200, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": Buffer.byteLength(result.response),
    "Access-Control-Allow-Origin": "*",
  });
  response.end(result.response);
}

const server = http.createServer((request, response) => {
  handle(request, response).catch((error) => {
    sendJSON(response, 500, { code: 500, errno: 1, msg: error.message });
  });
});

server.listen(listenPort, listenHost, () => {
  console.log(`DH macOS bridge: http://${listenHost}:${listenPort}`);
  console.log(`旺商聊 DevTools: ${devtoolsURL}`);
});
