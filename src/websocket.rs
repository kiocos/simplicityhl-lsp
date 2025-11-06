//! WebSocket server for the LSP
//!
//! This module provides a WebSocket bridge that allows web clients to connect
//! to the LSP server. Each WebSocket connection gets its own LSP backend instance.

use crate::backend::Backend;
use futures_util::{SinkExt, StreamExt};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tower_lsp_server::{LspService, Server};

/// Starts a WebSocket LSP server on the given address
pub async fn start_websocket_server(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(addr).await?;
    log::info!("WebSocket LSP server listening on {}", addr);

    let connection_count = Arc::new(Mutex::new(0usize));

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let connection_count = Arc::clone(&connection_count);

        tokio::spawn(async move {
            {
                let mut count = connection_count.lock().await;
                *count += 1;
                log::info!("New connection from {} (total: {})", peer_addr, *count);
            }

            if let Err(e) = handle_connection(stream).await {
                log::error!("Error handling connection from {}: {}", peer_addr, e);
            }

            {
                let mut count = connection_count.lock().await;
                *count -= 1;
                log::info!(
                    "Connection closed from {} (remaining: {})",
                    peer_addr,
                    *count
                );
            }
        });
    }
}

/// Handles a single WebSocket connection
async fn handle_connection(stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    // Accept WebSocket handshake
    let ws_stream = accept_async(stream).await?;
    log::debug!("WebSocket handshake completed");

    // Split WebSocket into sender and receiver
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    // Create channels for communication with LSP
    let (client_tx, mut client_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (server_tx, server_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Create async I/O adapters
    let stdin = ChannelReader::new(server_rx);
    let stdout = ChannelWriter::new(client_tx);

    // Create LSP service
    let (service, socket) = LspService::new(Backend::new);

    // Spawn LSP server task
    let lsp_task = tokio::spawn(async move {
        Server::new(stdin, stdout, socket).serve(service).await;
    });

    // Task: WebSocket -> LSP
    let ws_to_lsp_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    log::debug!("WS -> LSP: {}", text);

                    // Convert JSON to LSP format (add Content-Length header)
                    let lsp_message = format!("Content-Length: {}\r\n\r\n{}", text.len(), text);

                    if server_tx.send(lsp_message.into_bytes()).is_err() {
                        log::error!("Failed to send to LSP server");
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    log::debug!("WebSocket close received");
                    break;
                }
                Ok(_) => {
                    // Ignore binary, ping, pong
                }
                Err(e) => {
                    log::error!("WebSocket receive error: {}", e);
                    break;
                }
            }
        }
    });

    // Task: LSP -> WebSocket (with proper message framing)
    let lsp_to_ws_task = tokio::spawn(async move {
        let mut buffer: Vec<u8> = Vec::new();

        while let Some(data) = client_rx.recv().await {
            // Accumulate data from LSP
            buffer.extend_from_slice(&data);

            // Extract and forward all complete messages
            loop {
                match extract_one_lsp_message(&buffer) {
                    Ok(Some((json, consumed))) => {
                        log::debug!("LSP -> WS: {}", json);

                        if let Err(e) = ws_sender.send(Message::Text(json)).await {
                            log::error!("WebSocket send error: {}", e);
                            return;
                        }

                        // Remove the consumed bytes and continue parsing
                        buffer.drain(0..consumed);
                    }
                    Ok(None) => {
                        // Need more data
                        break;
                    }
                    Err(e) => {
                        log::error!("Failed to parse LSP message from buffer: {}", e);
                        // In case of parse error, drop buffer to resync
                        buffer.clear();
                        break;
                    }
                }
            }
        }
    });

    // Wait for any task to complete
    tokio::select! {
        _ = ws_to_lsp_task => log::debug!("WS to LSP task completed"),
        _ = lsp_to_ws_task => log::debug!("LSP to WS task completed"),
        _ = lsp_task => log::debug!("LSP task completed"),
    }

    Ok(())
}

/// Extract exactly one LSP message from the front of `buf` if complete.
/// Returns Ok(Some((json, consumed_bytes))) when a full message is available,
/// Ok(None) when more data is needed, or Err on malformed data.
fn extract_one_lsp_message(buf: &[u8]) -> Result<Option<(String, usize)>, String> {
    // Find header terminator
    let text = std::str::from_utf8(buf).map_err(|e| e.to_string())?;
    let header_end = match text.find("\r\n\r\n") {
        Some(i) => i,
        None => return Ok(None), // need more data
    };

    // Parse headers slice
    let headers = &text[..header_end];

    // Find and parse Content-Length
    let mut content_length: Option<usize> = None;
    for line in headers.split("\r\n") {
        // Trim and check case-insensitively for robustness
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            let value = rest.trim();
            content_length = Some(value.parse::<usize>().map_err(|e| e.to_string())?);
            break;
        }
    }

    let content_length = content_length.ok_or_else(|| "Missing Content-Length header".to_string())?;

    let body_start = header_end + 4; // skip CRLFCRLF
    let needed = body_start + content_length;
    if buf.len() < needed {
        return Ok(None); // body incomplete
    }

    let body_bytes = &buf[body_start..needed];
    let json = std::str::from_utf8(body_bytes)
        .map_err(|e| format!("Invalid UTF-8 body: {}", e))?
        .to_string();

    Ok(Some((json, needed)))
}

// Channel-based async I/O adapters

struct ChannelReader {
    rx: Arc<Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
    buffer: Vec<u8>,
    pos: usize,
}

impl ChannelReader {
    fn new(rx: mpsc::UnboundedReceiver<Vec<u8>>) -> Self {
        Self {
            rx: Arc::new(Mutex::new(rx)),
            buffer: Vec::new(),
            pos: 0,
        }
    }
}

impl AsyncRead for ChannelReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // If we have buffered data, use it first
        if self.pos < self.buffer.len() {
            let remaining = &self.buffer[self.pos..];
            let to_copy = std::cmp::min(buf.remaining(), remaining.len());
            buf.put_slice(&remaining[..to_copy]);
            self.pos += to_copy;

            if self.pos >= self.buffer.len() {
                self.buffer.clear();
                self.pos = 0;
            }

            return Poll::Ready(Ok(()));
        }

        // Try to receive new data
        let rx = Arc::clone(&self.rx);
        let mut rx_guard = match rx.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Poll::Pending,
        };

        match rx_guard.poll_recv(cx) {
            Poll::Ready(Some(data)) => {
                let to_copy = std::cmp::min(buf.remaining(), data.len());
                buf.put_slice(&data[..to_copy]);

                // Buffer any remaining data
                if to_copy < data.len() {
                    self.buffer = data[to_copy..].to_vec();
                    self.pos = 0;
                }

                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())), // EOF
            Poll::Pending => Poll::Pending,
        }
    }
}

struct ChannelWriter {
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

impl ChannelWriter {
    fn new(tx: mpsc::UnboundedSender<Vec<u8>>) -> Self {
        Self { tx }
    }
}

impl AsyncWrite for ChannelWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match self.tx.send(buf.to_vec()) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(_) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Channel closed",
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(Ok(()))
    }
}
