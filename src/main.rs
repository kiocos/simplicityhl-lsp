#![warn(clippy::all, clippy::pedantic)]

mod backend;
mod completion;
mod error;
mod function;
mod utils;
mod websocket;

use backend::Backend;
use tower_lsp_server::{LspService, Server};

#[tokio::main]
async fn main() {
    env_logger::init();

    // Check if we should run in WebSocket mode
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--websocket" {
        let addr = args.get(2).map(|s| s.as_str()).unwrap_or("127.0.0.1:9257");
        println!("Starting SimplicityHL LSP WebSocket server on {}", addr);
        println!("Press Ctrl+C to stop");

        if let Err(e) = websocket::start_websocket_server(addr).await {
            eprintln!("WebSocket server error: {}", e);
            std::process::exit(1);
        }
    } else {
        // Standard stdio mode
        let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());
        let (service, socket) = LspService::new(Backend::new);
        Server::new(stdin, stdout, socket).serve(service).await;
    }
}
