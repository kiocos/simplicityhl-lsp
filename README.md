# SimplicityHL LSP

Language Server for [SimplicityHL language](https://simplicity-lang.org/).

## Features

- Basic diagnostic for SimplicityHL code

![diagnostics](assets/diagnostics.gif)

- Completions of built-ins, jets and functions

![completion](assets/completion.gif)

- Hover for built-ins, jets and functions, with support of documentation

![hover](assets/hover.gif)

- Go to definition for functions

![goto-definition](assets/goto-definition.gif)

## Installation

Clone this repository and install using Cargo:

```bash
git clone https://github.com/distributed-lab/simplicityhl-lsp
cd simplicityhl-lsp
cargo install --path .
```

## Usage

### Standard Mode (stdio)

The LSP server runs in standard stdio mode by default, suitable for text editor integrations like VSCode, Neovim, etc.:

```bash
simplicityhl-lsp
```

### WebSocket Mode (for Web IDEs)

For web-based IDEs and WASM frontends, run the server in WebSocket mode:

```bash
# Start on default address (127.0.0.1:9257)
simplicityhl-lsp --websocket

# Or specify a custom address
simplicityhl-lsp --websocket 0.0.0.0:9257

# With debug logging
RUST_LOG=debug simplicityhl-lsp --websocket
```

Then connect from your web application:

```javascript
const ws = new WebSocket("ws://127.0.0.1:9257");
ws.onopen = () => {
  // Send LSP initialize request
  ws.send(
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        /* ... */
      },
    })
  );
};
```

For detailed WebSocket integration instructions, see [WEBSOCKET_USAGE.md](WEBSOCKET_USAGE.md).
