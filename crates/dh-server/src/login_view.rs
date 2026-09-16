//! Authenticated, bounded remote view of the official login page only.
//! Neither CDP commands nor page URLs are accepted from the browser.
use super::{authorized, same_origin, Server};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{tungstenite::Message as CdpMessage, MaybeTlsStream, WebSocketStream};
static ACTIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
type Cdp = WebSocketStream<MaybeTlsStream<TcpStream>>;
const STATUS: &str = "({login:location.protocol==='file:'&&location.hash.split('?')[0]==='#/login'&&!!window.electronAPI&&!window.nim,width:innerWidth,height:innerHeight,ready:!!window.nim})";

pub async fn upgrade(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if s.rust_mode {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    }
    if !same_origin(&s, &headers) || !authorized(&s, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(guard) = ACTIVE.try_lock() else {
        return StatusCode::CONFLICT.into_response();
    };
    ws.max_message_size(8192).on_upgrade(move |mut socket|async move {
        let _guard=guard;
        if run(&mut socket,s,headers).await.is_err(){let _=socket.send(Message::Text(json!({"type":"unavailable","message":"登录映射已停止，请确认官方客户端停留在登录页并重试"}).to_string().into())).await;}
        let _=socket.send(Message::Close(None)).await;
    })
}
async fn rpc(cdp: &mut Cdp, id: &mut u64, method: &str, params: Value) -> Result<Value, ()> {
    *id += 1;
    let request = *id;
    cdp.send(CdpMessage::Text(
        json!({"id":request,"method":method,"params":params})
            .to_string()
            .into(),
    ))
    .await
    .map_err(|_| ())?;
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(frame) = cdp.next().await {
            let frame = frame.map_err(|_| ())?;
            if let CdpMessage::Text(text) = frame {
                let response: Value = serde_json::from_str(&text).map_err(|_| ())?;
                if response["id"].as_u64() == Some(request) {
                    return if response.get("error").is_some() {
                        Err(())
                    } else {
                        Ok(response["result"].clone())
                    };
                }
            }
        }
        Err(())
    })
    .await
    .map_err(|_| ())?
}
async fn status(cdp: &mut Cdp, id: &mut u64) -> Result<Value, ()> {
    Ok(rpc(
        cdp,
        id,
        "Runtime.evaluate",
        json!({"expression":STATUS,"returnByValue":true}),
    )
    .await?["result"]["value"]
        .clone())
}
fn login_page(page: &Value) -> bool {
    let Some(raw) = page["url"].as_str() else {
        return false;
    };
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    page["type"] == "page"
        && url.scheme() == "file"
        && url.path().ends_with("/dist/index.html")
        && url
            .fragment()
            .is_some_and(|h| h.split('?').next() == Some("/login"))
}
fn local_socket(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|u| {
        u.scheme() == "ws"
            && u.host_str() == Some("127.0.0.1")
            && u.port() == Some(9222)
            && u.path().starts_with("/devtools/page/")
    })
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Input {
    Pointer { action: String, x: f64, y: f64 },
    Text { text: String },
    Key { key: String },
    Wheel { x: f64, y: f64, delta: f64 },
}
fn input_command(
    input: Input,
    width: f64,
    height: f64,
    pressed: &mut bool,
) -> Result<(&'static str, Value), ()> {
    let coordinate = |x: f64, y: f64| {
        x.is_finite() && y.is_finite() && x >= 0. && y >= 0. && x < width && y < height
    };
    match input {
        Input::Pointer { action, x, y } => {
            if !coordinate(x, y) {
                return Err(());
            }
            let ty = match action.as_str() {
                "down" => {
                    *pressed = true;
                    "mousePressed"
                }
                "up" => {
                    *pressed = false;
                    "mouseReleased"
                }
                "move" => "mouseMoved",
                _ => return Err(()),
            };
            Ok((
                "Input.dispatchMouseEvent",
                json!({"type":ty,"x":x,"y":y,"button":if *pressed||action=="up"{"left"}else{"none"},"buttons":if *pressed{1}else{0},"clickCount":if action=="move"{0}else{1}}),
            ))
        }
        Input::Text { text } if !text.is_empty() && text.len() <= 4096 => {
            Ok(("Input.insertText", json!({"text":text})))
        }
        Input::Key { key } => {
            let code = match key.as_str() {
                "Backspace" => 8,
                "Tab" => 9,
                "Enter" => 13,
                "Escape" => 27,
                "ArrowLeft" => 37,
                "ArrowUp" => 38,
                "ArrowRight" => 39,
                "ArrowDown" => 40,
                "Delete" => 46,
                _ => return Err(()),
            };
            Ok((
                "Input.dispatchKeyEvent",
                json!({"type":"keyDown","key":key,"windowsVirtualKeyCode":code}),
            ))
        }
        Input::Wheel { x, y, delta }
            if coordinate(x, y) && delta.is_finite() && delta.abs() <= 2000. =>
        {
            Ok((
                "Input.dispatchMouseEvent",
                json!({"type":"mouseWheel","x":x,"y":y,"deltaX":0,"deltaY":delta}),
            ))
        }
        _ => Err(()),
    }
}
async fn run(socket: &mut WebSocket, s: Arc<Server>, headers: HeaderMap) -> Result<(), ()> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(4))
        .build()
        .map_err(|_| ())?;
    let pages: Vec<Value> = client
        .get("http://127.0.0.1:9222/json/list")
        .send()
        .await
        .map_err(|_| ())?
        .json()
        .await
        .map_err(|_| ())?;
    let page = pages.iter().find(|p| login_page(p)).ok_or(())?;
    let address = page["webSocketDebuggerUrl"]
        .as_str()
        .filter(|a| local_socket(a))
        .ok_or(())?;
    let (mut cdp, _) = tokio_tungstenite::connect_async(address)
        .await
        .map_err(|_| ())?;
    let mut id = 0;
    let mut pressed = false;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let started = tokio::time::Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(600) || !authorized(&s, &headers) {
            break;
        }
        tokio::select! {
            incoming=socket.recv()=>{
                let Some(Ok(Message::Text(text)))=incoming else{break};
                let input:Input=serde_json::from_str(&text).map_err(|_|())?;
                let current=status(&mut cdp,&mut id).await?;
                if current["login"]!=true{break}
                let (method,params)=input_command(input,current["width"].as_f64().ok_or(())?,current["height"].as_f64().ok_or(())?,&mut pressed)?;
                rpc(&mut cdp,&mut id,method,params.clone()).await?;
                if method=="Input.dispatchKeyEvent" {let mut up=params;up["type"]=json!("keyUp");rpc(&mut cdp,&mut id,method,up).await?;}
            },
            _=tick.tick()=>{
                let before=status(&mut cdp,&mut id).await?;
                if before["login"]!=true {
                    socket.send(Message::Text(json!({"type":if before["ready"]==true{"complete"}else{"stopped"}}).to_string().into())).await.map_err(|_|())?;break
                }
                let frame=rpc(&mut cdp,&mut id,"Page.captureScreenshot",json!({"format":"jpeg","quality":65,"captureBeyondViewport":false})).await?;
                let after=status(&mut cdp,&mut id).await?;
                if after["login"]!=true{continue}
                let image=frame["data"].as_str().filter(|v|v.len()<4*1024*1024).ok_or(())?;
                socket.send(Message::Text(json!({"type":"frame","image":image,"width":after["width"],"height":after["height"]}).to_string().into())).await.map_err(|_|())?;
            }
        }
    }
    // Release drag if a browser disconnects during a slider gesture, only on login page.
    if pressed
        && status(&mut cdp, &mut id)
            .await
            .is_ok_and(|v| v["login"] == true)
    {
        let _ = rpc(
            &mut cdp,
            &mut id,
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":0,"y":0,"button":"left","buttons":0,"clickCount":1}),
        )
        .await;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restricts_targets_and_input() {
        assert!(login_page(
            &json!({"type":"page","url":"file:///app/dist/index.html#/login?logout=1"})
        ));
        assert!(!login_page(
            &json!({"type":"page","url":"file:///app/dist/index.html#/home"})
        ));
        assert!(!local_socket("ws://example.com:9222/devtools/page/a"));
        let mut pressed = false;
        assert!(input_command(
            Input::Pointer {
                action: "down".into(),
                x: 5.,
                y: 5.
            },
            100.,
            100.,
            &mut pressed
        )
        .is_ok());
        assert!(pressed);
        assert!(input_command(Input::Key { key: "F12".into() }, 100., 100., &mut pressed).is_err());
        assert!(input_command(
            Input::Pointer {
                action: "down".into(),
                x: 101.,
                y: 5.
            },
            100.,
            100.,
            &mut pressed
        )
        .is_err());
        assert!(serde_json::from_value::<Input>(
            json!({"type":"evaluate","expression":"anything"})
        )
        .is_err());
    }
}
