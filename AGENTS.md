# Zephyr（泽帆）开发代理指南

给在此仓库做日常迭代开发的 AI 代理阅读的约定。**先读这份文档再动手。**

## 项目是什么

免 Docker daemon 直接经 Registry HTTP API v2 拉取镜像，导出为 `docker save`(v1) 布局的 `.tar`。Tauri(v2, Rust) 后端 + React/TypeScript/Vite 前端。

## 技术栈与目录

```text
src/            前端（React 19 + TS + Vite）
src-tauri/src/  后端（Rust，模块见下）
  endpoint.rs  镜像名解析（registry/repo/tag/digest）
  auth.rs      Bearer token 认证
  registry.rs   Registry v2 客户端
  info.rs      V1 ID / ChainID 计算
  config.rs    OCI config blob 解析
  collect.rs   docker save v1 内容收集
  output.rs    并发下载 + 打包 tar
  pull.rs      `pull_image` 命令编排 + 进度事件
```

## 日常开发约定

### 前端（src/）

- 用 React 函数组件 + hooks（`useState`/`useEffect`/`useCallback`），类型用 `interface`。

- 业务文案用中文；`localStorage` 的 key 加 `dpt.` 前缀，读写封装成 `load/save` 助手。

- 后端调用：`invoke("pull_image", {...})`；进度经 `listen('pull://progress')` 订阅。

- 改界面样式保持在 `App.css`，遵循现有 `.topbar/.panel/.chip/.state` 等类与视觉基调。

- 包装错误时复用 `friendlyError()` 的中文归因（404/网络/清单/架构），不要直接抛英文栈。

### 后端（src-tauri/src/）

- 每个模块用 `//!` 说明与上游 docker-tar 的对应关系；**复刻上游行为的字节级细节必须写注释**（如 collect.rs 里非末层 digest 的 os 语义）。

- 错误统一 `Result<T, String>`，用 `format!("...: {e}")` 包装，错误文案可读、面向用户（中文）。

- 涉及上游复刻的逻辑新增/改动，必须在同一文件 `#[cfg(test)] mod tests` 内补测试。

- 结构化数据用 `#[derive(...)]` 明确；返回给前端的结果体用 `#[derive(Serialize)]`。

## 工作流与原则

- **先核心后辅助**：优先打通主流程（解析→认证→manifest→下载→打包），再补边界/体验。

- 不依赖本地 docker daemon，测试走单元测试与真实 registry（如 `nginx`）。

- 改动后必须验证：`npm run tauri dev` 跑通、后端 `cargo test` 通过。

- 提交前 `git status` 自查，只暂存相关文件，别带入构建产物。

## 提交约定

- **提交信息用中文**描述（说明「做了什么 / 为什么」，简洁聚焦）。

- **提交前检查隐私内容**：确认暂存内容里没有密钥、token、凭据、本地绝对路径或个人信息（如 `.env`、私钥、含用户名/密码的配置）。

- **不主动推送**：除非用户明确授意，否则只做本地 `git commit`，不执行 `git push`。

