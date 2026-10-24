# Skybin

A fast, dark-mode desktop app for managing your S3 buckets. Works with AWS S3 and any S3-compatible storage: MinIO, Cloudflare R2, Wasabi, Garage, and more.

## Features

- **Saved connections**: store as many accounts as you like (endpoint, region, keys, path-style) and switch between them in one click
- **Buckets**: create and delete buckets, optionally emptying them first
- **Browse**: navigate folders with breadcrumbs, then sort, filter and multi-select (Ctrl/Shift)
- **Transfer**: upload files or whole folders with the button or drag & drop, and download files or folders
- **Organize**: rename, move, create folders and delete objects
- **Details**: see each object's content type, ETag, storage class and custom `x-amz-meta-*` metadata

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Enter` | Open folder |
| `Backspace` | Go up one folder |
| `F2` | Rename |
| `F5` | Refresh |
| `Del` | Delete |
| `Ctrl+A` | Select all |
| `Esc` | Clear selection |

## Install

Install [Rust](https://rustup.rs), then run:

```sh
cargo install --git https://github.com/realChriss/skybin
```

Then start it with `skybin`.

## Security note

On Windows, your access keys are encrypted with Windows DPAPI, so only your Windows account on this computer can read them (stored in `%APPDATA%skybin`). On macOS and Linux they are currently saved **unencrypted**. Either way, prefer keys with limited permissions.

## License

[Apache 2.0](LICENSE)

Want to build or contribute? See [DEVELOPMENT.md](DEVELOPMENT.md).
