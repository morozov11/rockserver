//! Strict fallback for legacy HTTP radio sources using an ICY status line.

use std::time::Duration;

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Url;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

use super::{MAX_HEADERS, icy::Reader, security};

/// Opens a validated plain-HTTP source and accepts only `ICY 200 OK`.
pub(super) async fn open_icy(url: &Url, allow_loopback: bool) -> Result<(HeaderMap, Reader), ()> {
    if url.scheme() != "http" {
        return Err(());
    }
    let (_, address) = security::checked_endpoint(url, allow_loopback).await?;
    let mut socket = timeout(Duration::from_secs(10), TcpStream::connect(address))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    let host = &url[url::Position::BeforeHost..url::Position::AfterPort];
    let target = &url[url::Position::BeforePath..];
    let request = format!(
        "GET {target} HTTP/1.1\r\nHost: {host}\r\nIcy-MetaData: 1\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    timeout(
        Duration::from_secs(10),
        socket.write_all(request.as_bytes()),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;

    let mut bytes = Vec::new();
    let header_end = loop {
        if bytes.len() >= MAX_HEADERS {
            return Err(());
        }
        let mut buffer = [0; 1024];
        let count = timeout(Duration::from_secs(10), socket.read(&mut buffer))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        if count == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    if header_end > MAX_HEADERS {
        return Err(());
    }
    let head = std::str::from_utf8(&bytes[..header_end]).map_err(|_| ())?;
    let mut lines = head[..head.len() - 4].split("\r\n");
    if lines.next() != Some("ICY 200 OK") {
        return Err(());
    }
    let mut headers = HeaderMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(())?;
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| ())?;
        let value = HeaderValue::from_str(value.trim()).map_err(|_| ())?;
        headers.append(name, value);
    }
    // The legacy path streams identity bytes; unsupported transfer coding must not corrupt audio.
    if headers.contains_key("transfer-encoding") {
        return Err(());
    }
    Ok((
        headers,
        Reader::from_icy(socket, bytes[header_end..].to_vec()),
    ))
}
