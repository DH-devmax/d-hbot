//! 共享的 HTTP 响应体读取工具。
//!
//! 所有读取远端/公网响应体的地方都应该走这里，不要直接 `bytes().await` 或 `.json()`。
//! 先把整个响应收进内存再检查长度，上限就形同虚设——那时内存已经吃完了。
//! 这里按 chunk 累加，一超限立刻返回并断开连接。
//!
//! 本模块只负责"读多少"，不负责"怎么报错"：错误以 [`CappedReadError`] 返回，
//! 由调用方映射到自己模块的错误码，避免把某一个域的错误码泄漏到其他域。

/// 响应体读取失败的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CappedReadError {
    /// 传输中断或连接错误。
    Transport,
    /// 累计长度超过上限，已提前停止读取。
    TooLarge,
}

/// 默认响应体上限。
///
/// 这是一个宽松的天花板，不是调优参数：要关掉的缺陷是"无上限"，不是"上限太大"。
/// 例如 BCLC 的 Keno 年度归档本身就是整年开奖数据的 ZIP，有若干 MB，
/// 所以不能按"JSON 应该很小"去收紧这个值。
pub(crate) const DEFAULT_RESPONSE_LIMIT: usize = 20 * 1024 * 1024;

/// 边读边限长地取回响应体。
///
/// 一旦累计长度超过 `limit` 就立即返回 [`CappedReadError::TooLarge`]，
/// 此时 `response` 被丢弃，连接随之关闭，剩余数据不会再进内存。
pub(crate) async fn read_capped_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, CappedReadError> {
    let mut response = response;
    let mut body = Vec::new();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|_| CappedReadError::Transport)?;
        let Some(chunk) = chunk else { break };
        if body.len() + chunk.len() > limit {
            return Err(CappedReadError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// 起一个只应答一次的本地服务，返回 (地址, worker)。
    ///
    /// Content-Length 按 `body` 的真实长度填写；调用方超限断开时，
    /// 这里的写入可能是 broken pipe，属预期，所以忽略写入结果。
    fn serve_once(body: String) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 8 * 1024];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{address}"), worker)
    }

    async fn get(url: &str) -> reqwest::Response {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(url)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn aborts_when_response_exceeds_limit() {
        // 钉住"超限立刻停"，避免有人把它改回先收全再判长度。
        let (url, worker) = serve_once("x".repeat(16 * 1024));
        let error = read_capped_body(get(&url).await, 1024).await.unwrap_err();
        assert_eq!(error, CappedReadError::TooLarge);
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn returns_payload_within_limit() {
        let (url, worker) = serve_once("hello".into());
        let body = read_capped_body(get(&url).await, 1024).await.unwrap();
        assert_eq!(body.as_slice(), b"hello");
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn accepts_body_exactly_at_limit() {
        // 边界：等于上限必须放过，只有严格超过才拒。
        let (url, worker) = serve_once("y".repeat(1024));
        let body = read_capped_body(get(&url).await, 1024).await.unwrap();
        assert_eq!(body.len(), 1024);
        worker.join().unwrap();
    }
}
