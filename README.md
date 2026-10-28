# PDF Reader for Green star

A minimal PDF reader for **Green star**, the custom Zed fork, on macOS Apple Silicon. Read documents in an editor tab with smooth scrolling, zoom and Fit width. Pages are rendered at the display's resolution as they become visible.

This extension provides reading only: no selection tools, agent attachments, OCR, annotation editor or browser dependency. Encrypted PDFs are not supported.

## Install

Requires a Green star build that supports `paged_document_v1`.

1. Open the command palette and choose **Zed: Install Extension from GitHub**.
2. Enter `https://github.com/unteku3/green-star-pdf-reader`.
3. Review the release and choose **Install and enable**.
4. Open a local PDF file.

Use the mouse wheel or trackpad to scroll, **Cmd+Plus / Cmd+Minus** to zoom, and **Cmd+0** for Fit width. Disable, enable or uninstall it in **Extensions**. If the older PDF Viewer is installed, disable it before using PDF Reader.

This is a native extension for Green star, not an extension for the official Zed marketplace. Its executable runs with your user permissions. Downloads use HTTPS and SHA-256 verification. Release binaries are ad-hoc signed, not Apple-notarized.

## Build and verify

Rust and macOS Command Line Tools are required. The renderer uses the operating system's CoreGraphics PDF engine; it does not download a rendering library.

```sh
cargo test --workspace --locked
cargo build --release --locked
python3 tests/ipc.py target/release/green-star-pdf-reader
python3 scripts/package.py
```

Publish both files from `dist/` as GitHub Release assets: `viewer-aarch64-apple-darwin.zgv` and `viewer-aarch64-apple-darwin.zgv.sha256`. Their names are part of Green star's installer format.

The process owns PDF handles and renders requested tiles directly into bounded BGRA buffers. The host owns scrolling, tile scheduling and the raster cache. Opening the same file in multiple views shares its native document; closing the last view releases it. Tests generate their own PDFs; no personal documents belong in this repository.

See [PROTOCOL.md](PROTOCOL.md) for the interface and bounds. GPL-3.0-or-later; the vendored protocol comes from the Green star fork. See [LICENSE](LICENSE).
