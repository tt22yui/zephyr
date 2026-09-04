# 泽帆 Zephyr · 开发计划

> 按「先核心后辅助」的原则推进，每项完成后更新此处状态。
> 遵循 [AGENTS.md](AGENTS.md) 的工程约定（中文注释、`Result<T, String>` 错误、后端补单测）。

***

## 1. 镜像搜索功能

### 目标

在应用内直接搜索镜像仓库（首期覆盖 Docker Hub），点击结果即可一键填入「镜像名称」并发起拉取，替代手拼镜像名 + 记忆示例的体验。

### 现状

- 输入镜像名目前只能靠手动输入或 `EXAMPLES` 示例 chip、历史记录（`dpt.history`）。

- 后端已有完整的 registry v2 拉取链路（[endpoint.rs](src-tauri/src/endpoint.rs) 解析 → [auth.rs](src-tauri/src/auth.rs) 认证 → [registry.rs](src-tauri/src/registry.rs) 抓 manifest → [output.rs](src-tauri/src/output.rs) 下载打包），搜索只需新增一个「查仓库列表」入口，不触碰主流程。

### 方案设计

#### 后端（新增 `src-tauri/src/search.rs`）

- 新增模块 `search.rs`，走 **Docker Hub 公开搜索 API**（即 `docker search` 同源接口）：
  `GET https://hub.docker.com/v2/search/repositories/?query={q}&page_size={n}`

- 结构体（`#[derive(Serialize)]` 返回给前端）：

  - `SearchResult { name, description, stars, is_official, updated_at }`

  - `repo_name`（如 `nginx` / `user/repo`）直接作为可拉取的镜像名，官方仓库即 `library/*`。

- 命令 `search_image(query: String, page_size: Option<u32>) -> Result<Vec<SearchResult>, String>`：

  - query 为空直接返回空列表；page\_size 默认 25、上限 100。

  - 复用 `reqwest::Client`（可与 registry.rs 共用一个 client 或独立构造）。

  - 错误统一 `format!("...: {e}")` 包装，文案面向用户（中文），可由前端 `friendlyError()` 归因。

- 在 [lib.rs](src-tauri/src/lib.rs) 的 `invoke_handler` 注册 `search::search_image`。

- `#[cfg(test)] mod tests`：离线单测 JSON 解析（含 `is_official` 缺省、空 results）、URL 构造与 query 编码。

> 说明：Registry v2 规范的 `/_catalog` 只支持私有 registry 且不做关键词搜索，不具备通用搜索能力，因此首期不做自定义 registry 搜索。

#### 前端（`src/App.tsx` + `src/App.css`）

- 在「镜像名称」输入框下方新增搜索区：

  - 搜索输入框 + 「搜索」按钮，回车触发；`busy`（拉取中）时禁用。

  - 结果列表：仓库名（mono）、简介、⭐ star 数、「官方」徽标；点击结果把仓库名填入镜像输入框（并收起列表）。

  - 加载态（spinner）、错误态（复用 `friendlyError()` 中文归因）、空结果提示。

- 状态：`searching`、`searchResults: SearchResult[]`、`searchError`；用 `useCallback` 封装 `searchImage()` 调 `invoke("search_image", ...)`。

- 搜索历史可存 `localStorage`（`dpt.search`），只存成功搜索过的关键词，复用现有 `load/save` 助手。

- 样式沿用现有 `.panel/.chip/.mono/.spinner` 视觉基调，在 `App.css` 补 `.search-*` 类。

#### 数据流

```
输入关键词 → invoke("search_image") → hub.docker.com/v2/search/repositories
                                         ↓
                            结果列表（点击）→ 填入镜像名 → 走现有 pull_image 主流程
```

### 实施步骤

1. ✅ 后端：新建 `search.rs`（HTTP 请求 + 反序列化 + 错误包装 + 单测），注册命令，`cargo test` 通过（54 passed）。
2. ✅ 前端：App.tsx 增加搜索状态与 UI（搜索框、结果列表、点击回填、`dpt.search` 搜索历史），App.css 增加 `.search-*` 样式；`tsc --noEmit` 与 `vite build` 通过。
3. ⬜ 端到端验证：`npm run tauri dev` 搜索 `nginx`，点击结果发起拉取，验证成功。

### 风险与边界

- Docker Hub 搜索 API 为公开接口、无需认证，但属第三方服务：请求失败/超时按网络错误归因处理，不阻塞现有拉取流程。

- 搜索只覆盖 Docker Hub；GHCR 等第三方 registry 各有独立搜索 API，留作后续扩展（不在本项范围）。

- 结果名填入后仍需用户确认 tag/架构，搜索不代填 tag（默认走 `latest` 语义）。

### 完成标准

- [ ] `search_image` 命令可返回搜索结果，`cargo test` 全绿。

- [ ] 界面可搜索、可点击回填、可发起拉取；错误与空态友好。

- [ ] `npm run tauri dev` 手动验证通过。

***

## 待办/后续（尚未排期）

- <br />

  1. 自定义 registry 搜索（`_catalog` 或各平台独立搜索 API）

- <br />

  1. 镜像标签（tag）浏览

- <br />

  1. （占位，按需补充）

