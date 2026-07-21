package cdpbridge

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"
	"time"
)

var errNIMNotReady = errors.New("NIM 未就绪：请先登录旺商聊并等待会话初始化")

const maxBodySize = 8 << 20

var forwardedGroupRoutes = map[string]bool{
	"/v1/group/get-group-list":      true,
	"/v1/group/get-group-members":   true,
	"/v1/group/set-group-mute":      true,
	"/v1/group/set-member-mute":     true,
	"/v1/group/member-mute-cancel":  true,
	"/v1/group/set-member-nickname": true,
	"/v1/group/remove-group-member": true,
	"/v1/group/message-rollback":    true,
}

type Server struct {
	CDP *CDPClient
}

type ipcResult struct {
	TransportCode *int   `json:"transportCode"`
	Errno         int    `json:"errno"`
	Response      string `json:"response"`
	Error         string `json:"error"`
}

type deliveryResult struct {
	OK           bool   `json:"ok"`
	ErrorCode    string `json:"errorCode"`
	ErrorMessage string `json:"errorMessage"`
	IDClient     string `json:"idClient"`
	IDServer     string `json:"idServer"`
	Status       string `json:"status"`
	Flow         string `json:"flow"`
	Scene        string `json:"scene"`
}

var enumValues = map[string]map[string]int{
	"msgDevice": {"System": 0, "Desktop": 1, "Ios": 2, "Android": 3},
	"msgSession": {
		"MSG_KIND_SYSTEM": 0, "MSG_KIND_P2P": 1, "MSG_KIND_GROUP": 2,
		"MSG_KIND_GROUP_SYSTEM": 3, "MSG_KIND_NOTIFICATION": 4,
		"MSG_KIND_GROUP_NOTIFICATION": 5, "MSG_BATCH_ATTACH__NOTIFICATION": 6,
		"MSG_BROADCAST_SYSTEM": 7,
	},
	"accountType": {
		"ACCOUNT_MEMBER": 0, "ACCOUNT_CUSTOMER_SERVICE": 1,
		"ACCOUNT_FINANCE_SERVICE": 2, "ACCOUNT_MERCHANT": 3,
		"ACCOUNT_THIRD_SERVICE": 4, "ACCOUNT_SYSTEM": 5, "ACCOUNT_INNER_USER": 6,
	},
	"msgFormat": {
		"MSG_TEXT": 0, "MSG_IMG": 1, "MSG_VOICE": 2, "MSG_VIDEO": 3,
		"MSG_GEO": 4, "MSG_FILE": 6, "MSG_GROUP_NOTIFICATION": 7,
		"MSG_GROUP_NOTICE": 8, "MSG_FORWARD": 9, "MSG_RED_ENVELOPE": 10,
		"MSG_TRANSFER": 11, "MSG_PLUGINS": 12, "MSG_FRIEND_CARD": 13,
	},
	"msgRole":     {"MSG_MINE": 0, "MSG_MASTER": 1, "MSG_ADMIN": 2, "MSG_MEMBER": 3, "MSG_SYSTEM": 4},
	"msgRingtone": {"MSG_RINGTONE_NONE": 0},
	"appoint":     {"MSG_APPOINT_NONE": 0, "MSG_APPOINT_QUOTE": 1, "MSG_APPOINT_FORWARD": 2},
}

func NewServer(devtoolsURL string) *Server {
	return &Server{CDP: NewCDPClient(devtoolsURL)}
}

func (s *Server) Handler() http.Handler {
	return http.HandlerFunc(s.handle)
}

func (s *Server) handle(response http.ResponseWriter, request *http.Request) {
	host, _, _ := net.SplitHostPort(request.RemoteAddr)
	address := net.ParseIP(host)
	if address == nil || !address.IsLoopback() {
		sendJSON(response, http.StatusForbidden, map[string]any{"code": 403, "errno": 1, "msg": "loopback access only"})
		return
	}
	if request.Method == http.MethodOptions {
		sendJSON(response, http.StatusForbidden, map[string]any{"code": 403, "errno": 1, "msg": "browser preflight is disabled"})
		return
	}
	if (request.URL.Path == "/ping" && request.Method != http.MethodGet) ||
		(request.URL.Path != "/ping" && request.Method != http.MethodPost) {
		response.Header().Set("Allow", allowedMethod(request.URL.Path))
		sendJSON(response, http.StatusMethodNotAllowed, map[string]any{"code": 405, "errno": 1, "msg": "method not allowed"})
		return
	}
	ctx, cancel := context.WithTimeout(request.Context(), 25*time.Second)
	defer cancel()
	if err := s.route(ctx, response, request); err != nil {
		status, code, errno := http.StatusInternalServerError, 500, 1
		if errors.Is(err, errNIMNotReady) {
			status, code, errno = http.StatusServiceUnavailable, 503, 2
		}
		sendJSON(response, status, map[string]any{
			"code": code, "errno": errno, "msg": err.Error(),
		})
	}
}

func (s *Server) route(ctx context.Context, response http.ResponseWriter, request *http.Request) error {
	path := request.URL.Path
	if path == "/ping" {
		var page map[string]any
		if err := s.evaluate(ctx, `({title:document.title,route:location.hash,ready:document.readyState})`, &page); err != nil {
			return err
		}
		page["bridge"] = "native"
		sendJSON(response, http.StatusOK, map[string]any{"code": 0, "errno": 0, "msg": "OK", "data": page})
		return nil
	}

	payload, err := readPayload(request)
	if err != nil {
		return err
	}
	switch path {
	case "/v1/plugins/session-info":
		info, err := s.sessionInfo(ctx)
		if err != nil {
			return err
		}
		sendJSON(response, http.StatusOK, map[string]any{
			"code": 0, "errno": 0, "msg": "OK", "data": info,
		})
		return nil
	case "/v1/plugins/listen-msg":
		var state map[string]any
		if err := s.evaluate(ctx, listenerExpression(), &state); err != nil {
			return err
		}
		sendJSON(response, http.StatusOK, map[string]any{
			"code": 0, "errno": 0, "msg": "OK", "data": state,
		})
		return nil
	case "/v1/plugins/poll-msg", "/v1/plugins/peek-msg":
		var state map[string]any
		if err := s.evaluate(ctx, listenerExpression(), &state); err != nil {
			return err
		}
		var batch map[string]any
		if err := s.evaluate(ctx, pollExpression(), &batch); err != nil {
			return err
		}
		sendJSON(response, http.StatusOK, map[string]any{
			"code": 0, "errno": 0, "msg": "OK", "data": batch,
		})
		return nil
	case "/v1/plugins/ack-msg":
		seq, err := uint64Value(payload["seq"])
		if err != nil || seq == 0 {
			return fmt.Errorf("ack-msg seq 格式错误")
		}
		var ack map[string]any
		if err := s.evaluate(ctx, ackExpression(seq), &ack); err != nil {
			return err
		}
		sendJSON(response, http.StatusOK, map[string]any{
			"code": 0, "errno": 0, "msg": "OK", "data": ack,
		})
		return nil
	case "/v1/plugins/send-msg":
		return s.sendMessage(ctx, response, payload)
	case "/v1/plugins/nim-team-members":
		groupID := payload["groupId"]
		target, err := s.resolveGroupTarget(ctx, groupID)
		if err != nil {
			return err
		}
		var result map[string]any
		if err := s.evaluate(ctx, nimTeamMembersExpression(target), &result); err != nil {
			return err
		}
		if ok, _ := result["ok"].(bool); !ok {
			sendJSON(response, http.StatusBadGateway, map[string]any{"code": 502, "errno": 1, "msg": firstValue(stringValue(result["errorMessage"]), "NIM 群成员读取失败")})
			return nil
		}
		sendJSON(response, http.StatusOK, map[string]any{"code": 0, "errno": 0, "msg": "OK", "data": result})
		return nil
	case "/v1/plugins/nim-update-nick":
		target, err := s.resolveGroupTarget(ctx, payload["groupId"])
		if err != nil {
			return err
		}
		nimID, nick := stringValue(payload["nimId"]), stringField(payload, "nick")
		if nimID == "" || strings.TrimSpace(nick) == "" {
			return fmt.Errorf("nim-update-nick 缺少 nimId/nick")
		}
		var result map[string]any
		if err := s.evaluate(ctx, nimUpdateNickExpression(target, nimID, nick), &result); err != nil {
			return err
		}
		if ok, _ := result["ok"].(bool); !ok {
			sendJSON(response, http.StatusBadGateway, map[string]any{"code": 502, "errno": 1, "msg": firstValue(stringValue(result["errorMessage"]), "NIM 群名片修改失败")})
			return nil
		}
		sendJSON(response, http.StatusOK, map[string]any{"code": 0, "errno": 0, "msg": "OK", "data": result})
		return nil
	default:
		if !forwardedGroupRoutes[path] {
			sendJSON(response, http.StatusNotFound, map[string]any{"code": 404, "errno": 1, "msg": "route not found"})
			return nil
		}
		var result ipcResult
		if err := s.evaluate(ctx, ipcExpression("request", path, payload), &result); err != nil {
			return err
		}
		if result.TransportCode == nil || *result.TransportCode != 200 || result.Response == "" {
			sendJSON(response, http.StatusBadGateway, map[string]any{
				"code": transportCode(result.TransportCode), "errno": result.Errno,
				"msg": firstValue(result.Error, "xclient 请求失败"),
			})
			return nil
		}
		response.Header().Set("Content-Type", "application/json; charset=utf-8")
		response.WriteHeader(http.StatusOK)
		_, _ = io.WriteString(response, result.Response)
		return nil
	}
}

func allowedMethod(path string) string {
	if path == "/ping" {
		return http.MethodGet
	}
	return http.MethodPost
}

func (s *Server) sessionInfo(ctx context.Context) (map[string]any, error) {
	var runtime struct {
		NIMAccount string `json:"nimAccount"`
	}
	if err := s.evaluate(ctx, `({nimAccount:String((window.nim&&(window.nim.account||window.nim.options&&window.nim.options.account||window.nim.config&&window.nim.config.account))||"")})`, &runtime); err != nil {
		return nil, err
	}
	if runtime.NIMAccount == "" {
		return nil, errNIMNotReady
	}
	var groupsResult ipcResult
	if err := s.evaluate(ctx, ipcExpression("request", "/v1/group/get-group-list", map[string]any{}), &groupsResult); err != nil {
		return nil, err
	}
	if groupsResult.TransportCode == nil || *groupsResult.TransportCode != 200 || groupsResult.Response == "" {
		return nil, fmt.Errorf("群列表读取失败")
	}
	var groupsEnvelope struct {
		Code int `json:"code"`
		Data struct {
			Owner  []map[string]any `json:"owner"`
			Member []map[string]any `json:"member"`
		} `json:"data"`
	}
	if err := json.Unmarshal([]byte(groupsResult.Response), &groupsEnvelope); err != nil {
		return nil, fmt.Errorf("解析群列表: %w", err)
	}
	groups := append(groupsEnvelope.Data.Owner, groupsEnvelope.Data.Member...)
	if groupsEnvelope.Code != 0 || len(groups) == 0 {
		return nil, fmt.Errorf("当前账号没有可用群")
	}
	groupID := stringValue(groups[0]["groupId"])
	groupNumber, err := strconv.ParseInt(groupID, 10, 64)
	if err != nil || groupNumber == 0 {
		return nil, fmt.Errorf("默认群 ID 格式错误")
	}
	var membersResult ipcResult
	if err := s.evaluate(ctx, ipcExpression("request", "/v1/group/get-group-members", map[string]any{"groupId": groupNumber}), &membersResult); err != nil {
		return nil, err
	}
	if membersResult.TransportCode == nil || *membersResult.TransportCode != 200 || membersResult.Response == "" {
		return nil, fmt.Errorf("群成员读取失败")
	}
	var membersEnvelope struct {
		Code int `json:"code"`
		Data struct {
			Members []map[string]any `json:"groupMemberInfo"`
		} `json:"data"`
	}
	if err := json.Unmarshal([]byte(membersResult.Response), &membersEnvelope); err != nil {
		return nil, fmt.Errorf("解析群成员: %w", err)
	}
	if membersEnvelope.Code != 0 {
		return nil, fmt.Errorf("群成员读取失败")
	}
	var senderID int64
	for _, member := range membersEnvelope.Data.Members {
		if stringValue(member["nimId"]) != runtime.NIMAccount {
			continue
		}
		senderID, _ = strconv.ParseInt(stringValue(member["userId"]), 10, 64)
		break
	}
	if senderID == 0 {
		return nil, fmt.Errorf("当前账号未映射到群成员")
	}
	return map[string]any{
		"senderId": senderID, "defaultGroupId": groupNumber,
		"groupCount": len(groups), "nimReady": true, "nimAccount": runtime.NIMAccount,
	}, nil
}

func (s *Server) sendMessage(ctx context.Context, response http.ResponseWriter, payload map[string]any) error {
	message, ok := objectField(payload, "msg")
	if !ok {
		return fmt.Errorf("send-msg 缺少 msg")
	}
	scene := stringField(payload, "scene")
	if scene == "" {
		scene = messageScene(message["msgSession"])
	}
	target := stringValue(payload["nimTarget"])
	if target == "" && scene == "team" {
		to, ok := objectField(message, "to")
		if !ok {
			return fmt.Errorf("send-msg 缺少目标")
		}
		var err error
		target, err = s.resolveGroupTarget(ctx, to["id"])
		if err != nil {
			return err
		}
	}
	if target == "" {
		return fmt.Errorf("send-msg 缺少 NIM 目标")
	}
	content, err := s.encode(ctx, message)
	if err != nil {
		return err
	}
	localOnly, _ := payload["localOnly"].(bool)
	var delivery deliveryResult
	if err := s.evaluate(ctx, nimSendExpression(scene, target, content, localOnly), &delivery); err != nil {
		return err
	}
	if !delivery.OK {
		sendJSON(response, http.StatusBadGateway, map[string]any{
			"code": 502, "errno": 1, "msg": firstValue(delivery.ErrorMessage, "NIM 投递失败"),
			"data": map[string]any{"errorCode": delivery.ErrorCode},
		})
		return nil
	}
	sendJSON(response, http.StatusOK, map[string]any{
		"code": 0, "errno": 0, "msg": "OK",
		"data": map[string]any{
			"delivered": true, "idClient": delivery.IDClient, "idServer": delivery.IDServer,
			"status": delivery.Status, "flow": delivery.Flow, "scene": delivery.Scene,
		},
	})
	return nil
}

func (s *Server) encode(ctx context.Context, message map[string]any) (string, error) {
	normalized, err := normalizeMessage(message)
	if err != nil {
		return "", err
	}
	var result ipcResult
	if err := s.evaluate(ctx, ipcExpression("encode", "/v1/plugins/encode-msg", normalized), &result); err != nil {
		return "", err
	}
	if result.TransportCode == nil || (*result.TransportCode != 0 && *result.TransportCode != 200) || result.Response == "" {
		return "", fmt.Errorf("encode 失败 (%d/%d): %s", transportCode(result.TransportCode), result.Errno, result.Error)
	}
	return result.Response, nil
}

func (s *Server) resolveGroupTarget(ctx context.Context, groupID any) (string, error) {
	var result ipcResult
	if err := s.evaluate(ctx, ipcExpression("request", "/v1/group/get-group-list", map[string]any{}), &result); err != nil {
		return "", err
	}
	if result.TransportCode == nil || *result.TransportCode != 200 || result.Response == "" {
		return "", fmt.Errorf("群列表读取失败")
	}
	var envelope struct {
		Code int    `json:"code"`
		Msg  string `json:"msg"`
		Data struct {
			Owner  []map[string]any `json:"owner"`
			Member []map[string]any `json:"member"`
		} `json:"data"`
	}
	if err := json.Unmarshal([]byte(result.Response), &envelope); err != nil {
		return "", fmt.Errorf("解析群列表: %w", err)
	}
	if envelope.Code != 0 {
		return "", fmt.Errorf("%s", firstValue(envelope.Msg, "群列表读取失败"))
	}
	want := stringValue(groupID)
	for _, group := range append(envelope.Data.Owner, envelope.Data.Member...) {
		if stringValue(group["groupId"]) == want {
			if target := stringValue(group["groupCloudId"]); target != "" {
				return target, nil
			}
		}
	}
	return "", fmt.Errorf("群 ID 未映射到 NIM 群")
}

func (s *Server) evaluate(ctx context.Context, expression string, destination any) error {
	raw, err := s.CDP.Evaluate(ctx, expression)
	if err != nil {
		return err
	}
	if err := json.Unmarshal(raw, destination); err != nil {
		return fmt.Errorf("decode Electron result: %w", err)
	}
	return nil
}

func normalizeMessage(source map[string]any) (map[string]any, error) {
	raw, err := json.Marshal(source)
	if err != nil {
		return nil, err
	}
	var message map[string]any
	if err := json.Unmarshal(raw, &message); err != nil {
		return nil, err
	}
	for field, values := range enumValues {
		if value, ok := message[field].(string); ok {
			if number, found := values[value]; found {
				message[field] = number
			}
		}
	}
	timestamp := message["created_at"]
	if timestamp == nil {
		timestamp = message["createdAt"]
	}
	if text, ok := timestamp.(string); ok {
		parsed, err := time.Parse(time.RFC3339Nano, text)
		if err != nil {
			return nil, fmt.Errorf("created_at 时间格式错误: %w", err)
		}
		message["createdAt"] = map[string]any{"seconds": parsed.Unix(), "nanos": parsed.Nanosecond()}
	}
	delete(message, "created_at")
	return message, nil
}

func ipcExpression(kind, route string, payload any) string {
	input, _ := json.Marshal(map[string]any{"type": kind, "route": route, "payload": payload})
	return fmt.Sprintf(`(async()=>{
const input=%s;const ipc=require("electron").ipcRenderer;
return new Promise(resolve=>{const channel="dh-bridge-"+Date.now()+"-"+Math.random();
const timer=setTimeout(()=>resolve({transportCode:504,errno:1,error:"IPC timeout"}),15000);
ipc.once(channel,(event,value)=>{clearTimeout(timer);if(input.type==="decode"){
let bytes=[];if(value instanceof ArrayBuffer)bytes=Array.from(new Uint8Array(value));
else if(ArrayBuffer.isView(value))bytes=Array.from(new Uint8Array(value.buffer,value.byteOffset,value.byteLength));
resolve({transportCode:200,errno:0,binary:bytes});return;}
resolve({transportCode:value&&value.code,errno:value&&value.errno,response:value&&value.response,error:value&&value.message});});
if(input.type==="request")ipc.send("xclient",{type:"request",requestId:channel,url:input.route,
excuteType:input.route==="/v1/user/login"?2:0,params:JSON.stringify(input.payload||{}),key:channel});
else if(input.type==="encode")ipc.send("xclient",{type:"encode",params:JSON.stringify(input.payload),key:channel});
else ipc.send("xclient",{type:"decode",params:JSON.stringify(input.payload),key:channel});});})()`, input)
}

func nimSendExpression(scene, target, content string, localOnly bool) string {
	input, _ := json.Marshal(map[string]any{
		"scene": scene, "target": target, "content": content, "localOnly": localOnly,
	})
	return fmt.Sprintf(`(async()=>{const input=%s;return new Promise(resolve=>{
if(!window.nim){resolve({ok:false,errorCode:"NIM_NOT_READY",errorMessage:"NIM 未就绪"});return;}
const timer=setTimeout(()=>resolve({ok:false,errorCode:"TIMEOUT",errorMessage:"NIM send timeout"}),15000);
window.nim.sendCustomMsg({scene:input.scene,to:input.target,content:input.content,isLocal:input.localOnly,
done:(error,message)=>{clearTimeout(timer);resolve({ok:!error,errorCode:error&&(error.code||error.name),
errorMessage:error&&(error.message||String(error)),idClient:message&&message.idClient,idServer:message&&message.idServer,
status:message&&message.status,flow:message&&message.flow,scene:message&&message.scene});}});});})()`, input)
}

func nimTeamMembersExpression(teamID string) string {
	input, _ := json.Marshal(map[string]any{"teamId": teamID})
	return fmt.Sprintf(`(async()=>{const input=%s;return new Promise(resolve=>{
if(!window.nim){resolve({ok:false,errorCode:"NIM_NOT_READY",errorMessage:"NIM 未就绪",members:[]});return;}
const timer=setTimeout(()=>resolve({ok:false,errorCode:"TIMEOUT",errorMessage:"NIM team members timeout",members:[]}),15000);
window.nim.getTeamMembers({teamId:input.teamId,done:(error,value)=>{clearTimeout(timer);
const source=Array.isArray(value)?value:value&&Array.isArray(value.members)?value.members:[];
const members=source.map(item=>({nimId:String(item.account||item.accid||""),cardName:item.nickInTeam||item.nick||"",type:item.type||item.memberType||"normal",joinTime:item.joinTime||0}));
resolve({ok:!error,errorCode:error&&(error.code||error.name),errorMessage:error&&(error.message||String(error)),members});}});});})()`, input)
}

func nimUpdateNickExpression(teamID, nimID, nick string) string {
	input, _ := json.Marshal(map[string]any{"teamId": teamID, "account": nimID, "nickInTeam": nick})
	return fmt.Sprintf(`(async()=>{const input=%s;return new Promise(resolve=>{
if(!window.nim){resolve({ok:false,errorCode:"NIM_NOT_READY",errorMessage:"NIM 未就绪"});return;}
const timer=setTimeout(()=>resolve({ok:false,errorCode:"TIMEOUT",errorMessage:"NIM update nick timeout"}),15000);
window.nim.updateNickInTeam({teamId:input.teamId,account:input.account,nickInTeam:input.nickInTeam,done:(error,value)=>{clearTimeout(timer);
resolve({ok:!error,errorCode:error&&(error.code||error.name),errorMessage:error&&(error.message||String(error)),member:value||null});}});});})()`, input)
}

func listenerExpression() string {
	return `(()=>{
const nim=window.nim;if(!nim)return{ok:false,error:"NIM_NOT_READY",queued:0};
if(!window.__dhBridgeMessages){
 const state={queue:[],seen:new Set(),installed:[],limit:5000,nextSeq:1,dropped:0};
 const collect=(value,source)=>{
  if(Array.isArray(value)){value.forEach(item=>collect(item,source));return;}
  if(!value||typeof value!=="object")return;
  const id=value.idClient||value.idServer||[value.time||Date.now(),value.from||"",value.to||""].join("-");
  if(state.seen.has(id))return;state.seen.add(id);
  if(state.seen.size>20000){const first=state.seen.values().next().value;state.seen.delete(first);}
  state.queue.push({seq:state.nextSeq++,source,idClient:value.idClient,idServer:value.idServer,scene:value.scene,
   from:value.from,to:value.to,time:value.time,type:value.type,flow:value.flow,content:value.content,attach:value.attach,
   custom:value.custom,msgFormat:value.msgFormat,mentions:value.mentions||value.aite,quote:value.quote,
   fromNick:value.fromNick,sessionId:value.sessionId});
  if(state.queue.length>state.limit){const overflow=state.queue.length-state.limit;
   state.queue.splice(0,overflow);state.dropped+=overflow;}
 };
 ["onmsg","onmsgs","onofflinemsgs","onroamingmsgs"].forEach(name=>{
  const original=nim.options&&nim.options[name];
  if(typeof original!=="function")return;
  nim.options[name]=function(...args){try{collect(args[0],name);}catch{}return original.apply(this,args);};
  state.installed.push(name);
 });
 window.__dhBridgeMessages=state;
}
const state=window.__dhBridgeMessages;state.limit=5000;
if(!Number.isSafeInteger(state.nextSeq)||state.nextSeq<1)state.nextSeq=1;
if(!Number.isSafeInteger(state.dropped)||state.dropped<0)state.dropped=0;
for(const item of state.queue){if(!Number.isSafeInteger(item.seq)||item.seq<1)item.seq=state.nextSeq++;
 else if(item.seq>=state.nextSeq)state.nextSeq=item.seq+1;}
if(state.queue.length>state.limit){const overflow=state.queue.length-state.limit;
 state.queue.splice(0,overflow);state.dropped+=overflow;}
return{ok:true,installed:state.installed,queued:state.queue.length,dropped:state.dropped};
})()`
}

func pollExpression() string {
	return `(async()=>{
const state=window.__dhBridgeMessages;if(!state)return{ok:false,error:"LISTENER_NOT_READY",messages:[]};
const batch=state.queue.slice(0,100);let common=window.__dhBridgeCommon||null;
try{if(!common){
 const main=Array.from(document.scripts).map(item=>item.src).find(name=>/\/main-[^/]+\.js(?:\?|$)/.test(name));
 if(main){const source=await(await fetch(main)).text();const match=source.match(/zh-cn-[a-z0-9]+\.js/);
  if(match){common=(await import(new URL(match[0],main).href)).D;window.__dhBridgeCommon=common;}}
 }}catch{}
const messages=[];
for(const item of batch){let decoded=null,decodeError="";
 if(common&&item.type==="custom"&&typeof item.content==="string"){
  try{decoded=await common.decodeMsg(item.content);
   if(decoded&&decoded.mentions===undefined&&decoded.aite!==undefined)decoded={...decoded,mentions:decoded.aite};
  }catch(error){decodeError=error&&error.message||String(error);}
 }
 messages.push({...item,decoded,decodeError});
}
return{ok:true,messages,remaining:state.queue.length,dropped:state.dropped||0};
})()`
}

func ackExpression(seq uint64) string {
	input, _ := json.Marshal(map[string]any{"seq": seq})
	return fmt.Sprintf(`(()=>{
const input=%s;const state=window.__dhBridgeMessages;
if(!state)return{ok:false,error:"LISTENER_NOT_READY",acked:0,dropped:0,remaining:0};
const before=state.queue.length;state.queue=state.queue.filter(item=>Number(item.seq)>input.seq);
return{ok:true,acked:before-state.queue.length,dropped:state.dropped||0,remaining:state.queue.length};
})()`, input)
}

func readPayload(request *http.Request) (map[string]any, error) {
	defer request.Body.Close()
	decoder := json.NewDecoder(io.LimitReader(request.Body, maxBodySize+1))
	var payload map[string]any
	if err := decoder.Decode(&payload); err != nil {
		if err == io.EOF {
			return map[string]any{}, nil
		}
		return nil, fmt.Errorf("JSON 格式错误: %w", err)
	}
	return payload, nil
}

func objectField(source map[string]any, field string) (map[string]any, bool) {
	value, ok := source[field].(map[string]any)
	return value, ok
}

func stringField(source map[string]any, field string) string {
	value, _ := source[field].(string)
	return value
}

func stringValue(value any) string {
	switch typed := value.(type) {
	case string:
		return typed
	case json.Number:
		return typed.String()
	case float64:
		return strconv.FormatFloat(typed, 'f', -1, 64)
	case int:
		return strconv.Itoa(typed)
	case int64:
		return strconv.FormatInt(typed, 10)
	default:
		return ""
	}
}

func uint64Value(value any) (uint64, error) {
	switch typed := value.(type) {
	case json.Number:
		return strconv.ParseUint(typed.String(), 10, 64)
	case float64:
		if typed < 0 || typed != float64(uint64(typed)) {
			return 0, fmt.Errorf("not an unsigned integer")
		}
		return uint64(typed), nil
	case string:
		return strconv.ParseUint(strings.TrimSpace(typed), 10, 64)
	case int:
		if typed < 0 {
			return 0, fmt.Errorf("not an unsigned integer")
		}
		return uint64(typed), nil
	case int64:
		if typed < 0 {
			return 0, fmt.Errorf("not an unsigned integer")
		}
		return uint64(typed), nil
	case uint64:
		return typed, nil
	default:
		return 0, fmt.Errorf("not an unsigned integer")
	}
}

func messageScene(value any) string {
	if value == "MSG_KIND_GROUP" || stringValue(value) == "2" {
		return "team"
	}
	return "p2p"
}

func transportCode(code *int) int {
	if code == nil {
		return http.StatusBadGateway
	}
	return *code
}

func firstValue(values ...string) string {
	for _, value := range values {
		if strings.TrimSpace(value) != "" {
			return value
		}
	}
	return ""
}

func sendJSON(response http.ResponseWriter, status int, value any) {
	response.Header().Set("Content-Type", "application/json; charset=utf-8")
	response.WriteHeader(status)
	_ = json.NewEncoder(response).Encode(value)
}
