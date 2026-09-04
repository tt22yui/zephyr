# Zephyr (泽帆)

> A gentle breeze sails your Docker images home — no daemon required.

**Pull Docker images directly from a registry without a local Docker daemon**, and export them as a legacy `docker save` (v1) `.tar` file that you can later import with `docker load -i`.

---

## Features

- 🌬️ **Daemon-less**: no docker daemon or docker CLI required; talks straight to the registry over pure HTTP
- ⚙️ **Concurrent downloads**: layers (by diff_id) are pulled in parallel, with shared layers soft-linked for reuse
- 🔑 **Private registry auth**: optional username / password (Bearer token flow)
- 🗂️ **Target architecture**: choose `amd64` / `arm64`
- 📦 **`docker save` v1 compatible**: produces the v1 layout; image ID matches the official chain computation
- 🖥️ **Native GUI** (Tauri + React) with live progress bar and logs

## How it works

```
Auth → manifest index → platform manifest → config blob
   → V1 layer ID chain → index + manifest.json → download layers (concurrent)
   → normalize timestamps → pack tar
```

## Quick Start

Enter an image name in the app and click **Pull Image**. Any of these forms works:

```text
nginx:latest              # default docker.io, equivalent to nginx:latest
ghcr.io/x/y:tag           # any registry host
name@sha256:…             # pull by digest (if the registry supports it)
```

After the pull finishes, the UI shows how to import it:

```text
docker load -i <path>.tar
```

You can import the `.tar` into any environment where Docker is installed.

## Build

Requirements: Node.js ≥ 18, Rust stable (plus the WebView2 toolchain that varies by platform).

```bash
npm install
npm run tauri dev       # run in development
npm run tauri build     # package installers (Windows: .msi/.exe, macOS: .dmg/.app, Linux: .deb/.rpm/.AppImage)
```

> Build artifacts are written to `src-tauri/target/release/bundle/` by default.

## Compatibility

- **Registries**: Docker Hub, GHCR, Quay, and any registry implementing the Registry HTTP API v2
- **Output format**: `docker save` v1 layout (`manifest.json` + `VERSION` + `config.<id>.json` + per-layer tars)
- **Platforms** (provided by Tauri): Windows / macOS / Linux; icon sets are generated per platform

## Contributing

Issues and pull requests are welcome. Licensed under the [MIT](LICENSE) license.

## License

[MIT](LICENSE) © Zephyr Contributors