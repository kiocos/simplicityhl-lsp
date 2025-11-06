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

    // Task: LSP -> WebSocket
    let lsp_to_ws_task = tokio::spawn(async move {
        while let Some(data) = client_rx.recv().await {
            // Parse LSP message (strip Content-Length header, extract JSON)
            if let Some(json) = parse_lsp_message(&data) {
                log::debug!("LSP -> WS: {}", json);

                if let Err(e) = ws_sender.send(Message::Text(json.to_string())).await {
                    log::error!("WebSocket send error: {}", e);
                    break;
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

/// Parses an LSP message by stripping the Content-Length header
fn parse_lsp_message(data: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(data).ok()?;

    // Find the end of headers (\r\n\r\n)
    let header_end = text.find("\r\n\r\n")?;
    let json_body = &text[header_end + 4..];

    Some(json_body)
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
