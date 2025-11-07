# HOW TO RUN


## Layout
- `simplicity-webide/` — the Web IDE
- `simplicityhl-lsp/` — the LSP server

## Clone the correct branches
If you don’t already have the repos, clone them side‑by‑side in this folder on the `satshack` branch:

```bash
git clone -b satshack https://github.com/kiocos/simplicity-webide.git
git clone -b satshack https://github.com/kiocos/simplicityhl-lsp.git
```

If you already have them, make sure you’re on the right branch:

```bash
cd simplicity-webide && git fetch && git checkout satshack && cd ..
cd simplicityhl-lsp && git fetch && git checkout satshack && cd ..
```

## Start the LSP (Terminal 1)
From `simplicityhl-lsp/`:

```bash
cd simplicityhl-lsp
RUST_LOG=debug cargo run -- --websocket
```

## Start the IDE (Terminal 2)
From `simplicity-webide/`:

```bash
cd simplicity-webide
nix develop
just open
```

This opens the IDE (via the `just open` recipe) in your browser, with the LSP reachable over WebSocket.

## Notes
- Ensure you have Nix installed to use `nix develop`.
- `just` is provided by the dev shell; run it after `nix develop`.
- Ensure Rust (and Cargo) are installed for building/running the LSP (`rustup` recommended).

