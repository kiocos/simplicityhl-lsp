# WebSocket LSP - Quick Start

## 🚀 Start the Server

```bash
# Development
cargo run -- --websocket

# Production
cargo build --release
./target/release/simplicityhl-lsp --websocket
```

The server will start on `ws://127.0.0.1:9257` by default.

## 📡 Connect from Your WebIDE

```typescript
// 1. Connect
const ws = new WebSocket("ws://127.0.0.1:9257");

// 2. Initialize LSP
ws.onopen = () => {
  ws.send(
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        processId: null,
        clientInfo: { name: "Simplicity WebIDE", version: "1.0.0" },
        capabilities: {
          textDocument: {
            completion: {},
            hover: {},
            definition: {},
          },
        },
        rootUri: null,
      },
    })
  );
};

// 3. Handle responses
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);

  if (msg.method === "textDocument/publishDiagnostics") {
    // Display errors/warnings from msg.params.diagnostics
    console.log("Diagnostics:", msg.params.diagnostics);
  }
};

// 4. After initialize response, send initialized notification
ws.send(
  JSON.stringify({
    jsonrpc: "2.0",
    method: "initialized",
    params: {},
  })
);

// 5. Open a document
ws.send(
  JSON.stringify({
    jsonrpc: "2.0",
    method: "textDocument/didOpen",
    params: {
      textDocument: {
        uri: "file:///example.simf",
        languageId: "simplicityhl",
        version: 1,
        text: "fn main() -> u32 { 42 }",
      },
    },
  })
);
```

## 🔄 Document Updates

Send this whenever the user types:

```typescript
ws.send(
  JSON.stringify({
    jsonrpc: "2.0",
    method: "textDocument/didChange",
    params: {
      textDocument: {
        uri: "file:///example.simf",
        version: 2, // increment on each change
      },
      contentChanges: [{ text: newCodeContent }],
    },
  })
);
```

**Tip**: Add a 300-500ms debounce to avoid flooding the server.

## 🎯 Request Completions

```typescript
ws.send(
  JSON.stringify({
    jsonrpc: "2.0",
    id: 2,
    method: "textDocument/completion",
    params: {
      textDocument: { uri: "file:///example.simf" },
      position: { line: 0, character: 10 },
    },
  })
);
```

## ℹ️ Request Hover Info

```typescript
ws.send(
  JSON.stringify({
    jsonrpc: "2.0",
    id: 3,
    method: "textDocument/hover",
    params: {
      textDocument: { uri: "file:///example.simf" },
      position: { line: 0, character: 10 },
    },
  })
);
```

## 📍 Go to Definition

```typescript
ws.send(
  JSON.stringify({
    jsonrpc: "2.0",
    id: 4,
    method: "textDocument/definition",
    params: {
      textDocument: { uri: "file:///example.simf" },
      position: { line: 5, character: 10 },
    },
  })
);
```

## ✅ What You Get

- **Real-time diagnostics** (syntax errors, type errors)
- **Code completion** for:
  - Built-in functions
  - Jets (e.g., `jet::add_32`)
  - Your custom functions
  - Modules (`jet`, `param`, `witness`)
- **Hover information** with type signatures and docs
- **Go to definition** for user-defined functions

## 🔍 Debugging

Enable verbose logging:

```bash
RUST_LOG=debug cargo run -- --websocket
```

You'll see all messages flowing between the WebSocket and LSP backend.

## 📚 Full Documentation

See [WEBSOCKET_USAGE.md](WEBSOCKET_USAGE.md) for complete LSP protocol details and advanced usage.
