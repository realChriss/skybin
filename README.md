# Skybin

A small, dark-mode desktop client for AWS S3 and S3-compatible storage (MinIO, R2, Wasabi, Garage, …), built with Rust + egui.

- Saved connections (endpoint, region, keys, path-style)
- Create and delete buckets (optionally emptying them first)
- Browse folders with breadcrumbs, sort, filter, and multi-select (Ctrl/Shift)
- Upload files or folders (button or drag & drop), download files or folders, rename/move, new folder, delete
- Object metadata panel (content type, ETag, storage class, `x-amz-meta-*`, …)

Shortcuts: `Del` delete · `F2` rename · `F5` refresh · `Enter` open · `Backspace` up · `Ctrl+A` select all · `Esc` clear selection.

Credentials are saved **unencrypted** in eframe's app storage (`%APPDATA%\skybin` on Windows).

## Run

```sh
cargo run --release
```

## Test

```sh
cargo test                       # backend unit tests + headless UI tests (egui_kittest), offline
S3_TEST_ENDPOINT=http://127.0.0.1:9000 cargo test -- --ignored   # end-to-end against a real server
```

You can start a throwaway server for the end-to-end test with `docker run --rm -p 9000:9000 minio/minio server /data`.

## Layout

```
src/
  main.rs        entry point, loads saved connections
  app.rs         state, async task/message loop, all actions
  backend.rs     S3 operations (rust-s3), runs on tokio
  tests.rs       headless UI tests driven via the accessibility tree
  ui/
    mod.rs       frame layout, shortcuts, drag & drop
    sidebar.rs   connections + bucket list
    browser.rs   toolbar, breadcrumbs, object table
    dialogs.rs   modal dialogs
    panels.rs    status bar, properties panel
    theme.rs     dark theme
```
