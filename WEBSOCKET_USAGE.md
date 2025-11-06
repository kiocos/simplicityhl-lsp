# WebSocket LSP Server Usage

This document explains how to use the SimplicityHL LSP server in WebSocket mode for web-based IDEs.

## Starting the WebSocket Server

### Basic Usage

```bash
# Start on default address (127.0.0.1:9257)
cargo run -- --websocket

# Or with a specific address
cargo run -- --websocket 0.0.0.0:9257

# With logging enabled
RUST_LOG=debug cargo run -- --websocket
```

### Production Build

```bash
# Build the binary
cargo build --release

# Run the WebSocket server
./target/release/simplicityhl-lsp --websocket 127.0.0.1:9257
```

## Connecting from Your WASM Frontend

### 1. Establish WebSocket Connection

```javascript
const ws = new WebSocket("ws://127.0.0.1:9257");

ws.onopen = () => {
  console.log("Connected to LSP server");
  // Send initialize request
  sendLspRequest({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      processId: null,
      clientInfo: {
        name: "Simplicity WebIDE",
        version: "1.0.0",
      },
      capabilities: {
        textDocument: {
          completion: { dynamicRegistration: false },
          hover: { dynamicRegistration: false },
          definition: { dynamicRegistration: false },
        },
      },
      rootUri: null,
    },
  });
};

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  handleLspResponse(message);
};

ws.onerror = (error) => {
  console.error("WebSocket error:", error);
};

ws.onclose = () => {
  console.log("Disconnected from LSP server");
};
```

### 2. Send LSP Messages

The server expects JSON-RPC 2.0 formatted messages as plain text over WebSocket:

```javascript
function sendLspRequest(message) {
  ws.send(JSON.stringify(message));
}
```

**Important**: Unlike stdio LSP, you don't need to add `Content-Length` headers. The WebSocket server handles that conversion automatically.

### 3. Handle LSP Responses

```javascript
function handleLspResponse(message) {
  if (message.method === "textDocument/publishDiagnostics") {
    // Handle diagnostics
    displayDiagnostics(message.params.diagnostics);
  } else if (message.result) {
    // Handle response to a request
    handleResult(message.id, message.result);
  } else if (message.error) {
    // Handle error
    console.error("LSP error:", message.error);
  }
}
```

## LSP Protocol Flow

### 1. Initialize

```javascript
// Client sends:
{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "processId": null,
        "capabilities": { /* capabilities */ },
        "rootUri": null
    }
}

// Server responds:
{
    "jsonrpc": "2.0",
    "id": 1,
    "result": {
        "capabilities": {
            "textDocumentSync": 1,
            "completionProvider": { "triggerCharacters": [":"] },
            "hoverProvider": true,
            "definitionProvider": true
        }
    }
}
```

### 2. Initialized Notification

```javascript
// Client sends:
{
    "jsonrpc": "2.0",
    "method": "initialized",
    "params": {}
}
```

### 3. Open Document

```javascript
{
    "jsonrpc": "2.0",
    "method": "textDocument/didOpen",
    "params": {
        "textDocument": {
            "uri": "file:///example.simf",
            "languageId": "simplicityhl",
            "version": 1,
            "text": "fn main() {}"
        }
    }
}
```

### 4. Document Changes

```javascript
{
    "jsonrpc": "2.0",
    "method": "textDocument/didChange",
    "params": {
        "textDocument": {
            "uri": "file:///example.simf",
            "version": 2
        },
        "contentChanges": [{
            "text": "fn main() -> u32 { 42 }"
        }]
    }
}
```

### 5. Receive Diagnostics

```javascript
// Server sends (automatically):
{
    "jsonrpc": "2.0",
    "method": "textDocument/publishDiagnostics",
    "params": {
        "uri": "file:///example.simf",
        "diagnostics": [
            {
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 10 }
                },
                "severity": 1,  // 1=Error, 2=Warning
                "message": "Expected expression of type `u32`"
            }
        ]
    }
}
```

### 6. Completions

```javascript
// Client requests:
{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "textDocument/completion",
    "params": {
        "textDocument": { "uri": "file:///example.simf" },
        "position": { "line": 0, "character": 10 }
    }
}

// Server responds:
{
    "jsonrpc": "2.0",
    "id": 2,
    "result": [
        {
            "label": "jet::add_32",
            "kind": 3,  // Function
            "detail": "fn jet::add_32(a: u32, b: u32) -> (bool, u32)",
            "documentation": "Addition with overflow flag"
        }
        // ... more completions
    ]
}
```

### 7. Hover Information

````javascript
// Client requests:
{
    "jsonrpc": "2.0",
    "id": 3,
    "method": "textDocument/hover",
    "params": {
        "textDocument": { "uri": "file:///example.simf" },
        "position": { "line": 1, "character": 20 }
    }
}

// Server responds:
{
    "jsonrpc": "2.0",
    "id": 3,
    "result": {
        "contents": {
            "kind": "markdown",
            "value": "```simplicityhl\nfn jet::add_32(a: u32, b: u32) -> (bool, u32)\n```\nAdds two 32-bit unsigned integers..."
        },
        "range": { /* range of the hovered symbol */ }
    }
}
````

### 8. Go to Definition

```javascript
// Client requests:
{
    "jsonrpc": "2.0",
    "id": 4,
    "method": "textDocument/definition",
    "params": {
        "textDocument": { "uri": "file:///example.simf" },
        "position": { "line": 5, "character": 10 }
    }
}

// Server responds:
{
    "jsonrpc": "2.0",
    "id": 4,
    "result": {
        "uri": "file:///example.simf",
        "range": {
            "start": { "line": 0, "character": 3 },
            "end": { "line": 0, "character": 15 }
        }
    }
}
```

## Architecture

```
┌─────────────────┐         WebSocket          ┌──────────────────┐
│  WASM Frontend  │ ◄────────────────────────► │  LSP WS Server   │
│  (Browser)      │     JSON-RPC Messages      │  (Rust)          │
└─────────────────┘                             └──────────────────┘
                                                         │
                                                         ▼
                                                ┌──────────────────┐
                                                │   LSP Backend    │
                                                │  (per connection)│
                                                └──────────────────┘
```

Each WebSocket connection gets its own LSP backend instance, so multiple users/tabs can connect simultaneously without interfering with each other.

## Key Differences from Stdio LSP

1. **No Content-Length Headers**: The WebSocket server handles this automatically
2. **Pure JSON**: Send and receive plain JSON-RPC messages as text frames
3. **Connection-based**: Each connection maintains its own state
4. **Simultaneous Connections**: Multiple clients can connect at once

## Debugging

Enable verbose logging to see all messages:

```bash
RUST_LOG=debug cargo run -- --websocket
```

This will show:

- Connection events
- All messages flowing between WebSocket and LSP
- Parsing/formatting operations

## CORS Considerations

If you need to connect from a web app on a different origin, you may need to:

1. Use a reverse proxy (nginx, Caddy) that adds CORS headers
2. Or modify the WebSocket server to accept connections from your origin

For local development, browsers typically allow `ws://localhost` connections from `http://localhost`.

## Security Notes

⚠️ **Important for Production**:

1. The current implementation has no authentication
2. Anyone who can connect to the WebSocket can use the LSP
3. For production, consider:
   - Adding authentication tokens
   - Rate limiting per connection
   - Sandboxing the LSP execution
   - Using TLS (`wss://` instead of `ws://`)

## Integration with Simplicity WebIDE

Your WebIDE should:

1. Establish WebSocket connection on app startup
2. Send `initialize` → `initialized` sequence
3. Send `didOpen` for each document
4. Send `didChange` on user edits (with debouncing)
5. Listen for `publishDiagnostics` to show errors
6. Request completions on user trigger (e.g., typing `:`)
7. Request hover on cursor hover events
8. Request definition on "Go to Definition" action

## Example: Minimal Integration

```typescript
class SimplicityhLspClient {
  private ws: WebSocket | null = null;
  private messageId = 1;

  async connect() {
    this.ws = new WebSocket("ws://127.0.0.1:9257");

    this.ws.onopen = () => this.initialize();
    this.ws.onmessage = (e) => this.handleMessage(JSON.parse(e.data));
  }

  private initialize() {
    this.send({
      jsonrpc: "2.0",
      id: this.messageId++,
      method: "initialize",
      params: {
        processId: null,
        capabilities: {
          /* ... */
        },
        rootUri: null,
      },
    });
  }

  openDocument(uri: string, text: string) {
    this.send({
      jsonrpc: "2.0",
      method: "textDocument/didOpen",
      params: {
        textDocument: { uri, languageId: "simplicityhl", version: 1, text },
      },
    });
  }

  updateDocument(uri: string, text: string, version: number) {
    this.send({
      jsonrpc: "2.0",
      method: "textDocument/didChange",
      params: {
        textDocument: { uri, version },
        contentChanges: [{ text }],
      },
    });
  }

  private send(message: any) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message));
    }
  }

  private handleMessage(message: any) {
    if (message.method === "textDocument/publishDiagnostics") {
      this.onDiagnostics(message.params);
    }
    // Handle other messages...
  }

  onDiagnostics(params: any) {
    // Update UI with diagnostics
  }
}
```
