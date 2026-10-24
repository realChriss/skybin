<div align="center">

<img src="assets/icon.png" alt="" width="112">

# Skybin

A fast, dark-mode desktop app for managing your S3 buckets.<br>
Works with AWS S3 and any S3-compatible storage: MinIO, Cloudflare R2, Wasabi, Garage, and more.

[![Release](https://img.shields.io/github/v/release/realChriss/skybin)](https://github.com/realChriss/skybin/releases/latest)
[![CI](https://github.com/realChriss/skybin/actions/workflows/ci.yml/badge.svg)](https://github.com/realChriss/skybin/actions/workflows/ci.yml)

</div>

## Features

- **Saved connections**: store as many accounts as you like (endpoint, region, keys, path-style) and switch between them in one click
- **Buckets**: create and delete buckets, optionally emptying them first
- **Browse**: navigate folders with breadcrumbs, then sort, filter and multi-select (Ctrl/Shift)
- **Transfer**: upload files or whole folders with the button or drag & drop, and download files or folders
- **Organize**: rename, move, create folders and delete objects
- **Details**: see each object's content type, ETag, storage class and custom `x-amz-meta-*` metadata

## Install

### Download

Grab the binary for Windows, Linux or macOS (Apple Silicon) from the [latest release](https://github.com/realChriss/skybin/releases/latest). On Linux and macOS, make it executable with `chmod +x` first.

### Build from source

Install [Rust](https://rustup.rs), then run:

```sh
cargo install --git https://github.com/realChriss/skybin
```

Then start it with `skybin`.

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

## Security

Saved access keys are encrypted so only your user account can read them (DPAPI on Windows, Keychain on macOS, Secret Service on Linux). On Linux without a Secret Service keyring, they are saved unencrypted. Prefer keys with limited permissions.

## Contributing

See [DEVELOPMENT.md](DEVELOPMENT.md) for building, testing and releasing.

## License

[Apache 2.0](LICENSE)
