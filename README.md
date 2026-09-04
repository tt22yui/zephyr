# 泽帆 Zephyr

> 一阵和风，把 Docker 镜像从仓库（harbor）轻轻运回本地。
> A gentle breeze sails your Docker images home — no daemon required.

**无需本地 Docker daemon**，直接通过 [Registry HTTP API v2](https://distribution.github.io/distribution/spec/api/) 拉取镜像，并导出为老式 `docker save`（v1）格式的 `.tar` 文件，之后用 `docker load -i` 即可导入。

Pull Docker images **directly from a registry without a local daemon**, and export them as a legacy `docker save` (v1) `.tar` that you can later import with `docker load -i`.

---

## 功能特性 / Features

- 🌬️ 轻量无依赖：不依赖 docker daemon / docker CLI，纯 HTTP 直连
  Daemon-less: talks straight to the registry over HTTP, no environment lock-in
- ⚙️ 并发下载：对不同 layer（diff_id）并发拉取，共享层自动软链复用
  Concurrent layer downloads with shared-layer soft-link reuse
- 🔑 私有仓库认证：可选用户名 / 密码（Bearer token 流程）
  Optional auth for private registries (Bearer token flow)
- 🗂️ 目标架构选择：`amd64` / `arm64`
  Target architecture selection
- 📦 兼容 `docker save` v1 结构：镜像 ID（v1 ID chain）与官方一致
  Produces `docker save` v1 layout; image ID matches the official chain computation
- 🖥️ 图形界面（Tauri + React）：实时进度条与日志
  Native GUI (Tauri + React) with live progress bar and logs

## 工作原理 / How it works

```
Auth → manifest index → platform manifest → config blob
   → V1 layer ID chain → index + manifest.json → download layers (concurrent)
   → normalize timestamps → pack tar
```

## 快速开始 / Quick Start

在应用内填写镜像名，点击「拉取镜像」即可。支持多种写法：

```text
nginx:latest              # 默认 docker.io，等价 nginx:latest
ghcr.io/x/y:tag           # 任意 registry 主机
name@sha256:…             # 按 digest 拉取（需 registry 支持）
```

拉取完成后，界面会给出以下信息：

```text
docker load -i <path>.tar
```

用它把 `.tar` 导入任意安装了 Docker 的环境。

## 构建 / Build

环境要求：Node.js ≥ 18、Rust stable（含 WebView2 依平台而定的工具链）。

```bash
npm install
npm run tauri dev       # 开发运行
npm run tauri build     # 打包安装包 (Windows: .msi/.exe, macOS: .dmg/.app, Linux: .deb/.rpm/.AppImage)
```

> 打包产物默认输出到 `src-tauri/target/release/bundle/`。

## 兼容性 / Compatibility

- 镜像仓库：Docker Hub、GHCR、Quay 以及任何实现 Registry HTTP API v2 的 registry
- 产物格式：`docker save` v1 布局（`manifest.json` + `VERSION` + `config.<id>.json` + 各 layer tar）
- 平台支持（由 Tauri 提供）：Windows / macOS / Linux；图标集已按各平台生成

## 参与贡献 / Contributing

欢迎提交 Issue 或 PR。[MIT](LICENSE) 许可。

## License

[MIT](LICENSE) © Zephyr Contributors