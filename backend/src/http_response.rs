use anyhow::{Context, Result};

/// Reads an HTTP response without trusting Content-Length or allowing a
/// chunked peer to grow the process indefinitely.
pub(crate) async fn read_bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        anyhow::bail!("{label}超过安全上限 {max_bytes} 字节");
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(max_bytes);
    let mut body = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("读取{label}失败"))?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            anyhow::bail!("{label}超过安全上限 {max_bytes} 字节");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn response_for(raw_response: &'static [u8]) -> reqwest::Response {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream.write_all(raw_response).await.unwrap();
        });
        let response = reqwest::get(format!("http://{address}/")).await.unwrap();
        server.await.unwrap();
        response
    }

    #[tokio::test]
    async fn rejects_declared_response_length_before_reading_the_body() {
        let response =
            response_for(b"HTTP/1.1 200 OK\r\ncontent-length: 17\r\n\r\n0123456789abcdefg").await;

        let error = read_bounded_body(response, 16, "测试响应")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("超过安全上限"));
    }

    #[tokio::test]
    async fn rejects_chunked_response_that_crosses_the_limit() {
        let response = response_for(
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n11\r\n0123456789abcdefg\r\n0\r\n\r\n",
        )
        .await;

        let error = read_bounded_body(response, 16, "测试响应")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("超过安全上限"));
    }
}
