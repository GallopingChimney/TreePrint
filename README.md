# TreePrint

A desktop tool for visualizing directory structures and searching files. What started as a simple script to print a copyable directory tree grew into a lightweight file search engine with filtering and color-coded results.

Built with Rust and [egui](https://github.com/emilk/egui).

![Rust](https://img.shields.io/badge/Rust-1.94%2B-orange)

## Features

### Tree View

- Recursive ASCII directory tree (just like the `tree` command, but copyable)
- Color-coded file sizes on a logarithmic scale (green → yellow → red)
- Optional line counts for text files
- File/folder/size totals at the bottom
- Click any entry to reveal it in Explorer
- Copy the entire tree to clipboard

### Search

- Substring and glob pattern matching (`*`, `?`)
- Case-sensitive / case-insensitive toggle
- Parallel search using up to 8 threads
- Respects `.gitignore` rules
- Results displayed in a table with path, size, and last access time
- Highlighted matches in results
- Copy all result paths to clipboard

### Shared

- Native folder picker dialog
- Configurable filters:
  - Hidden files (dotfiles)
  - `.git` directories
  - `node_modules`
  - `target` (Rust build dir)
  - `build` / `dist` / `out`
  - Object files (`.o`, `.obj`, `.pdb`, `.exe`, `.dll`, `.so`, `.dylib`)
  - Lock files (`Cargo.lock`, `package-lock.json`, `yarn.lock`, etc.)
- Adjustable color ramp upper bound (1 KB – 4096 GB)
- Background processing with cancellation — UI stays responsive

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) 1.94.0 or newer

### Build & Run

```bash
# Debug
cargo run

# Release (optimized)
cargo build --release
./target/release/tree_print.exe
```

## Project Structure

```
TreePrint/
├── Cargo.toml      # Dependencies and metadata
├── src/
│   ├── main.rs     # App state, UI, tree builder
│   └── search.rs   # Parallel search & glob matching
```

## Dependencies

| Crate | Purpose |
|---|---|
| [eframe](https://crates.io/crates/eframe) | egui application framework |
| [rfd](https://crates.io/crates/rfd) | Native file dialogs |
| [ignore](https://crates.io/crates/ignore) | Fast directory walking with gitignore support |
| [crossbeam-channel](https://crates.io/crates/crossbeam-channel) | Multi-threaded message passing |
| [egui_extras](https://crates.io/crates/egui_extras) | Table widget for search results |
