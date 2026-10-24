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

Your access keys are encrypted before they are saved, so only your user account on this computer can read them:

- **Windows**: Windows DPAPI (stored in `%APPDATA%\skybin`)
- **macOS**: the encryption key lives in your Keychain
- **Linux**: the encryption key lives in your Secret Service keyring (GNOME Keyring, KWallet, …). Without one, keys are saved unencrypted.

Prefer access keys with limited permissions either way.

## License

[Apache 2.0](LICENSE)

Want to build or contribute? See [DEVELOPMENT.md](DEVELOPMENT.md).
