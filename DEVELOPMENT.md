# Development

Skybin is built with Rust, [egui](https://github.com/emilk/egui)/eframe for the UI and [rust-s3](https://github.com/durch/rust-s3) on tokio for storage access.

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

Saved connections live in eframe's app storage under the app id `skybin` (`%APPDATA%\skybin` on Windows).
