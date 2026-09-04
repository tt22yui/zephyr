import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import "./App.css";

/* ================= 类型 ================= */

type Arch = "any" | "amd64" | "arm64";

interface RegistryCfg {
  id: string;
  name: string;
  host: string;
  searchUrl: string;
}
interface AccountCfg {
  registry: string;
  username: string;
  password: string;
}
interface SearchResult {
  name: string;
  description: string;
  stars: number;
  is_official: boolean;
  updated_at: string;
}
interface InspectPlatform {
  os: string;
  architecture: string;
  variant: string;
  digest: string;
  size: number;
}
interface InspectConfig {
  architecture: string;
  os: string;
  docker_version: string;
  created: string | null;
  env: string[];
  cmd: string[];
  working_dir: string;
}
interface InspectResult {
  image: string;
  tag: string;
  digest: string | null;
  platforms: InspectPlatform[];
  tags: string[];
  config: InspectConfig | null;
  layer_count: number;
  total_size: number;
}
interface PullResult {
  top_id: string;
  layer_count: number;
  tar_path: string;
  image: string;
}
interface ProgressPayload {
  name: string;
  done: number;
  total: number;
  message: string;
}

type PullView =
  | { kind: "running"; progress: ProgressPayload; log: string[] }
  | { kind: "error"; raw: string; message: string }
  | { kind: "result"; data: PullResult };

type Stage =
  | { kind: "home" }
  | { kind: "results"; query: string; sourceUrl: string | null }
  | { kind: "detail"; image: string }
  | { kind: "pull"; image: string; view: PullView; arch: string };

/* ================= 存储与工具 ================= */

const STORE = {
  arch: "dpt.arch",
  source: "dpt.source",
  history: "dpt.history",
  search: "dpt.search",
  registries: "dpt.registries",
  accounts: "dpt.accounts",
  downloadDir: "dpt.downloadDir",
};

function load<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : (JSON.parse(v) as T);
  } catch {
    return fallback;
  }
}
function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* ignore */
  }
}

const HUB_HOST = "registry-1.docker.io";

/** 预置的常用第三方镜像库（搜索地址留空：仅支持直接输入引用拉取）。 */
const REGISTRY_PRESETS: { name: string; host: string; searchUrl: string }[] = [
  { name: "GitHub (ghcr.io)", host: "ghcr.io", searchUrl: "" },
  { name: "Quay (quay.io)", host: "quay.io", searchUrl: "" },
  { name: "Google (gcr.io)", host: "gcr.io", searchUrl: "" },
  { name: "Kubernetes (registry.k8s.io)", host: "registry.k8s.io", searchUrl: "" },
];

function friendlyError(raw: string): string {
  if (/404/.test(raw)) {
    return "镜像或标签不存在（404）。该仓库可能只发布了特定标签，latest 不一定存在，请换一个存在的 tag 重试。";
  }
  if (/JSON/.test(raw) || /解析/.test(raw)) {
    return "无法解析 registry 返回的数据。可能是仓库不存在、或需要认证。请检查镜像名是否正确，私有仓库请在设置里配置账号。";
  }
  if (/连接|timed out|timeout|tls|证书|certificate/i.test(raw)) {
    return "网络连接失败。请确认网络/代理可用（本地 registry 可勾选“使用 HTTP”），然后重试。";
  }
  if (/manifest|清单|架构|architecture/.test(raw)) {
    return "清单或架构不匹配。请检查镜像是否支持你选择的目标架构。";
  }
  if (/denied|forbidden|403/i.test(raw)) {
    return "镜像库拒绝了访问（403 / DENIED）。常见原因：① 该仓库是私有的——请到「设置 → 私有账号」为对应镜像库（如 ghcr.io）配置账号与 Token/PAT；② 仓库路径或命名空间填错了，或该仓库不存在。请检查后重试。";
  }
  return raw;
}

/** 归一化 docker 主机别名到 API 主机。 */
function hostOf(ref: string): string {
  const s = ref.trim();
  const slash = s.indexOf("/");
  let first = "";
  if (slash > 0) {
    first = s.slice(0, slash);
  }
  if (slash < 0 || !(first.includes(".") || first.includes(":") || first.toLowerCase() === "localhost")) {
    return HUB_HOST;
  }
  const h = first.toLowerCase();
  if (
    h === "docker.io" ||
    h === "index.docker.io" ||
    h === "registry.hub.docker.com" ||
    h === "registry.docker.io" ||
    h === "reg-1.docker.io"
  ) {
    return HUB_HOST;
  }
  return h;
}

/** 输入是否更像是完整镜像引用（而非关键词），用于走「直接拉取」而非搜索。 */
function looksLikeRef(s: string): boolean {
  const t = s.trim();
  if (!t) return false;
  if (t.includes("@sha256:")) return true;
  const slash = t.indexOf("/");
  if (slash > 0) {
    const first = t.slice(0, slash);
    if (first.includes(".") || first.includes(":") || first.toLowerCase() === "localhost") return true;
    return true; // 多段路径（如 library/nginx）也算引用
  }
  if (t.includes(":")) return true; // nginx:1.25
  return false;
}

function humanBytes(n: number): string {
  if (n <= 0) return "未知";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function uid(): string {
  return Math.random().toString(36).slice(2, 10);
}

/* ================= 应用根组件 ================= */

function App() {
  const [arch, setArch] = useState<Arch>(() => load(STORE.arch, "amd64"));
  const [source, setSource] = useState<string>(() => load(STORE.source, "hub"));
  const [stage, setStage] = useState<Stage>({ kind: "home" });
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [registries, setRegistries] = useState<RegistryCfg[]>(() => load(STORE.registries, []));
  const [accounts, setAccounts] = useState<AccountCfg[]>(() => load(STORE.accounts, []));
  const [downloadDir, setDownloadDir] = useState<string>(() => load(STORE.downloadDir, ""));

  useEffect(() => save(STORE.arch, arch), [arch]);
  useEffect(() => save(STORE.source, source), [source]);
  useEffect(() => save(STORE.registries, registries), [registries]);
  useEffect(() => save(STORE.accounts, accounts), [accounts]);
  useEffect(() => save(STORE.downloadDir, downloadDir), [downloadDir]);

  // 首次未配置下载目录时，默认取系统「下载」目录。
  useEffect(() => {
    if (!load(STORE.downloadDir, "")) {
      invoke<string>("get_download_dir")
        .then((dir) => {
          if (dir) setDownloadDir(dir);
        })
        .catch(() => {
          /* 解析失败保持为空，回退到当前目录 */
        });
    }
  }, []);

  // 进度事件 → 更新 pull 阶段
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listen<ProgressPayload>("pull://progress", (ev) => {
      setStage((prev) =>
        prev.kind === "pull" && prev.view.kind === "running"
          ? {
              ...prev,
              view: {
                ...prev.view,
                progress: ev.payload,
                log: [...prev.view.log.slice(-29), ev.payload.message],
              },
            }
          : prev,
      );
    }).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  /** 查找与某镜像 registry host 匹配的账号。 */
  const accountFor = useCallback(
    (ref: string): AccountCfg | undefined => {
      const host = hostOf(ref);
      return accounts.find((a) => normalizedHost(a.registry) === host);
    },
    [accounts],
  );

  /** 发起实际拉取。输出到设置的「下载目录」，未配置时由后端回退到当前目录。 */
  const runPull = useCallback(
    async (image: string, targetArch: string, useHttp: boolean) => {
      const acc = accountFor(image);
      const rec = {
        image,
        arch: targetArch,
        useHttp: useHttp || null,
        username: acc?.username || null,
        password: acc?.password || null,
        outDir: downloadDir || null,
      };
      setStage({ kind: "pull", image, view: { kind: "running", progress: { name: "auth", done: 0, total: 1, message: "准备开始…" }, log: ["准备开始…"] }, arch: targetArch });
      try {
        const r = await invoke<PullResult>("pull_image", rec);
        setStage({ kind: "pull", image, view: { kind: "result", data: r }, arch: targetArch });
      } catch (err) {
        const raw = String(err);
        setStage({ kind: "pull", image, view: { kind: "error", raw, message: friendlyError(raw) }, arch: targetArch });
      }
    },
    [accountFor, downloadDir],
  );

  function pushHistory(img: string) {
    const next = [...load<string[]>(STORE.history, []).filter((h) => h !== img), img].slice(-8);
    save(STORE.history, next);
  }

  function pushSearchHistory(kw: string) {
    const next = [...load<string[]>(STORE.search, []).filter((h) => h !== kw), kw].slice(-8);
    save(STORE.search, next);
  }

  return (
    <div className="app">
      <TopBar
        busy={stage.kind === "pull" && stage.view.kind === "running"}
        pullMessage={stage.kind === "pull" && stage.view.kind === "running" ? stage.view.progress.message : ""}
        source={source}
        setSource={setSource}
        arch={arch}
        setArch={setArch}
        registries={registries}
        onSettings={() => setSettingsOpen(true)}
      />

      <main className="layout">
        {stage.kind === "home" && (
          <HomeView
            onSearch={(query) => {
              const src = resolveSource(source, registries);
              if (src.host === HUB_HOST) {
                if (looksLikeRef(query)) {
                  setStage({ kind: "detail", image: query.trim() });
                } else {
                  pushSearchHistory(query.trim());
                  setStage({ kind: "results", query: query.trim(), sourceUrl: null });
                }
              } else if (src.searchUrl) {
                pushSearchHistory(query.trim());
                setStage({ kind: "results", query: query.trim(), sourceUrl: src.searchUrl });
              } else {
                // 无搜索 API 的第三方库：把输入当作引用；缺主机前缀时补上该库主机。
                const img = hasHostPrefix(query.trim()) ? query.trim() : `${src.host}/${query.trim()}`;
                setStage({ kind: "detail", image: img });
              }
            }}
          />
        )}

        {stage.kind === "results" && (
          <ResultsView
            query={stage.query}
            sourceUrl={stage.sourceUrl}
            onSelect={(name) => setStage({ kind: "detail", image: name })}
            onPull={(name) => {
              pushHistory(name);
              void runPull(name, concreteArch(arch), false);
            }}
            onBack={() => setStage({ kind: "home" })}
          />
        )}

        {stage.kind === "detail" && (
          <DetailView
            image={stage.image}
            defaultArch={arch}
            account={accountFor(stage.image)}
            onDownload={(img, targetArch, useHttp) => {
              pushHistory(img);
              void runPull(img, targetArch, useHttp);
            }}
            onBack={() => setStage({ kind: "home" })}
          />
        )}

        {stage.kind === "pull" && (
          <PullView
            image={stage.image}
            view={stage.view}
            arch={stage.arch}
            onRetry={() => runPull(stage.image, stage.arch, false)}
            onBack={() => setStage({ kind: "home" })}
          />
        )}
      </main>

      <SettingsDrawer
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        registries={registries}
        setRegistries={setRegistries}
        accounts={accounts}
        setAccounts={setAccounts}
        downloadDir={downloadDir}
        setDownloadDir={setDownloadDir}
      />
    </div>
  );
}

/* ================= 顶栏 ================= */

interface TopBarProps {
  busy: boolean;
  pullMessage: string;
  source: string;
  setSource: (s: string) => void;
  arch: Arch;
  setArch: (a: Arch) => void;
  registries: RegistryCfg[];
  onSettings: () => void;
}

function TopBar({ busy, pullMessage, source, setSource, arch, setArch, registries, onSettings }: TopBarProps) {
  return (
    <header className="topbar">
      <div className="topbar-right">
        {busy && (
          <div className="busy">
            <span className="spinner" aria-hidden="true" />
            {pullMessage}
          </div>
        )}
        <select
          className="top-select"
          value={source}
          onChange={(e) => setSource(e.currentTarget.value)}
          aria-label="镜像库来源"
          title="镜像库来源"
        >
          {sourceOptions(registries).map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
        <select
          className="top-select"
          value={arch}
          onChange={(e) => setArch(e.currentTarget.value as Arch)}
          aria-label="目标架构"
          title="目标架构"
        >
          <option value="any">全部架构</option>
          <option value="amd64">amd64</option>
          <option value="arm64">arm64</option>
        </select>
        <button className="icon-btn" type="button" onClick={onSettings} title="设置（三方库 / 账号）" aria-label="设置">
          <span className="gear" aria-hidden="true">⚙</span>
        </button>
      </div>
    </header>
  );
}

/* ================= 主界面（home） ================= */

interface HomeProps {
  onSearch: (q: string) => void;
}

function HomeView({ onSearch }: HomeProps) {
  const [query, setQuery] = useState("");
  const searchHistory = load<string[]>(STORE.search, []);

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!query.trim()) return;
    onSearch(query);
  }

  return (
    <section className="home">
      <form className="home-search" onSubmit={submit}>
        <input
          className="mono home-input"
          value={query}
          onChange={(e) => setQuery(e.currentTarget.value)}
          spellCheck={false}
          autoFocus
          aria-label="镜像名或关键词"
        />
        <button className="primary home-go" type="submit" disabled={!query.trim()} title="搜索 / 拉取" aria-label="搜索 / 拉取">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <circle cx="11" cy="11" r="7" />
            <path d="m21 21-4.3-4.3" />
          </svg>
        </button>
      </form>

      {searchHistory.length > 0 && (
        <div className="home-history" aria-label="搜索历史">
          <span className="home-history-label">最近搜索</span>
          <div className="home-chips">
            {searchHistory.map((h) => (
              <button key={h} type="button" className="chip" onClick={() => onSearch(h)} title={`重新使用：${h}`}>
                {h}
              </button>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}

/* ================= 搜索结果（results） ================= */

interface ResultsProps {
  query: string;
  sourceUrl: string | null;
  onSelect: (name: string) => void;
  onPull: (name: string) => void;
  onBack: () => void;
}

function ResultsView({ query, sourceUrl, onSelect, onPull, onBack }: ResultsProps) {
  const [results, setResults] = useState<SearchResult[] | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError("");
    setResults(null);
    invoke<SearchResult[]>("search_image", { query, pageSize: 25, base: sourceUrl })
      .then((r) => alive && setResults(r))
      .catch((err) => alive && setError(friendlyError(String(err))))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [query, sourceUrl]);

  return (
    <div className="result-page">
      <div className="page-head">
        <button className="ghost" type="button" onClick={onBack}>
          ← 返回
        </button>
        <span className="page-title">
          搜索 “<span className="mono">{query}</span>”
        </span>
      </div>

      {loading && (
        <div className="search-status">
          <span className="spinner" aria-hidden="true" />
          搜索中…
        </div>
      )}
      {!loading && error && <p className="search-error">{error}</p>}
      {!loading && !error && (
        <>
          <div className="pin-card">
            <div className="pin-main">
              <span className="pin-badge">直接拉取</span>
              <code className="mono pin-name">{query}</code>
            </div>
            <div className="pin-actions">
              <button className="ghost" type="button" onClick={() => onPull(query)}>
                直接下载
              </button>
              <button className="ghost" type="button" onClick={() => onSelect(query)}>
                详情
              </button>
            </div>
          </div>

          {results === null || results.length === 0 ? (
            <p className="search-empty">未找到相关镜像，换个关键词试试。</p>
          ) : (
            <ul className="result-list">
              {results.map((r) => (
                <li key={r.name} className="result-item">
                  <button
                    type="button"
                    className="result-main"
                    onClick={() => onSelect(r.name)}
                    title={`查看 ${r.name} 详情`}
                  >
                    <span className="result-name mono">{r.name}</span>
                    {r.is_official && <span className="badge-official">官方</span>}
                    <span className="result-stars">⭐ {r.stars}</span>
                    <span className="result-desc">{r.description || "（无简介）"}</span>
                    {r.updated_at && <span className="result-updated">更新 {r.updated_at.slice(0, 10)}</span>}
                  </button>
                  <div className="result-actions">
                    <button className="ghost" type="button" onClick={() => onPull(r.name)} title="用默认参数立即下载">
                      直接下载
                    </button>
                    <button className="ghost" type="button" onClick={() => onSelect(r.name)}>
                      详情
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}

/* ================= 详情（detail） ================= */

interface DetailProps {
  image: string;
  defaultArch: Arch;
  account?: AccountCfg;
  onDownload: (img: string, arch: string, useHttp: boolean) => void;
  onBack: () => void;
}

function DetailView({ image, defaultArch, account, onDownload, onBack }: DetailProps) {
  const [inspect, setInspect] = useState<InspectResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [arch, setArch] = useState<string>(defaultArch === "any" ? "amd64" : defaultArch);
  const [useHttp, setUseHttp] = useState(false);

  // 重新检查：切换架构 / HTTP 时按该配置重新拉 config / 层信息
  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError("");
    setInspect(null);
    invoke<InspectResult>("inspect_image", {
      image,
      arch,
      useHttp,
      username: account?.username || null,
      password: account?.password || null,
    })
      .then((r) => alive && setInspect(r))
      .catch((err) => alive && setError(friendlyError(String(err))))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [image, arch, useHttp]);

  const platforms = inspect?.platforms ?? [];
  const archOptions = platforms.length > 0 ? platforms.map((p) => p.architecture) : ["amd64", "arm64"];
  const archs = Array.from(new Set(archOptions));

  // 检查完成后，若当前架构不在可用列表，则自动落到第一个可用架构。
  useEffect(() => {
    if (inspect && archs.length > 0 && !archs.includes(arch)) {
      setArch(archs[0]);
    }
  }, [inspect, arch, archs.join(",")]);

  return (
    <div className="result-page">
      <div className="page-head">
        <button className="ghost" type="button" onClick={onBack}>
          ← 返回
        </button>
        <span className="page-title mono">{image}</span>
      </div>

      {loading && (
        <div className="search-status">
          <span className="spinner" aria-hidden="true" />
          正在读取镜像信息…
        </div>
      )}
      {!loading && error && <p className="search-error">{error}</p>}

      {!loading && inspect && (
        <div className="detail">
          <section className="detail-meta panel-detail">
            <h2 className="detail-h2">镜像信息</h2>
            <dl className="kv">
              <dt>镜像</dt>
              <dd className="mono">{inspect.image}</dd>
              {inspect.digest && (
                <>
                  <dt>Digest</dt>
                  <dd className="mono">{inspect.digest}</dd>
                </>
              )}
              {inspect.config?.created && (
                <>
                  <dt>创建时间</dt>
                  <dd className="mono">{inspect.config.created}</dd>
                </>
              )}
              {inspect.config?.docker_version && (
                <>
                  <dt>Docker 版本</dt>
                  <dd className="mono">{inspect.config.docker_version}</dd>
                </>
              )}
              {inspect.platforms.length > 0 && (
                <>
                  <dt>可用架构</dt>
                  <dd className="mono">
                    {inspect.platforms
                      .map((p) => `${p.os}/${p.architecture}${p.variant ? "/" + p.variant : ""}`)
                      .join(" · ")}
                  </dd>
                </>
              )}
              <dt>层数</dt>
              <dd className="mono">{inspect.layer_count}</dd>
              <dt>总大小</dt>
              <dd className="mono">{humanBytes(inspect.total_size)}</dd>
            </dl>
          </section>

          {inspect.tags.length > 0 && (
            <section className="detail-tags panel-detail">
              <h2 className="detail-h2">标签（{inspect.tags.length}）</h2>
              <div className="home-chips">
                {inspect.tags.slice(0, 5).map((t) => (
                  <button
                    key={t}
                    type="button"
                    className="chip"
                    onClick={() => onSelectTag(image, t, onDownload)}
                    title={`拉取 ${splitBase(image)}:${t}`}
                  >
                    {t}
                  </button>
                ))}
                {inspect.tags.length > 5 && (
                  <span className="chip muted">…另有 {inspect.tags.length - 5} 个</span>
                )}
              </div>
            </section>
          )}

          {inspect.config && (inspect.config.cmd.length > 0 || inspect.config.env.length > 0 || inspect.config.working_dir) && (
            <details className="detail-config panel-detail">
              <summary>
                <h2 className="detail-h2">运行配置</h2>
              </summary>
              {inspect.config.working_dir && (
                <p className="cfg-line">
                  <span className="cfg-key">WORKDIR</span> <code className="mono">{inspect.config.working_dir}</code>
                </p>
              )}
              {inspect.config.cmd.length > 0 && (
                <p className="cfg-line">
                  <span className="cfg-key">CMD</span>{" "}
                  <code className="mono">{inspect.config.cmd.map((c) => (c.includes(" ") ? `"${c}"` : c)).join(" ")}</code>
                </p>
              )}
              {inspect.config.env.length > 0 && (
                <ul className="cfg-env">
                  {inspect.config.env.map((e) => (
                    <li key={e} className="mono">
                      {e}
                    </li>
                  ))}
                </ul>
              )}
            </details>
          )}

          <section className="detail-pull panel-detail">
            <h2 className="detail-h2">下载</h2>
            <div className="pull-params">
              <label className="field grow">
                <span className="field-label">目标架构</span>
                <select className="mono" value={arch} onChange={(e) => setArch(e.currentTarget.value)}>
                  {archs.map((a) => (
                    <option key={a} value={a}>
                      {a}
                    </option>
                  ))}
                </select>
              </label>
              <label className="toggle">
                <input type="checkbox" checked={useHttp} onChange={(e) => setUseHttp(e.currentTarget.checked)} />
                <span className="toggle-track" aria-hidden="true" />
                <span className="toggle-label">使用 HTTP</span>
              </label>
            </div>
            <button
              className="primary"
              type="button"
              onClick={() => onDownload(image, arch, useHttp)}
            >
              开始下载
            </button>
            <p className="hint">输出目录可在「设置 → 下载目录」配置，默认保存到系统下载目录。</p>
          </section>
        </div>
      )}
    </div>
  );
}

function splitBase(ref: string): string {
  const at = ref.indexOf("@");
  const base = at >= 0 ? ref.slice(0, at) : ref;
  const colon = base.lastIndexOf(":");
  const slash = base.lastIndexOf("/");
  if (colon > slash) return base.slice(0, colon);
  return base;
}

function onSelectTag(
  baseRef: string,
  tag: string,
  onDownload: (img: string, arch: string, useHttp: boolean) => void,
) {
  const base = splitBase(baseRef);
  const full = `${base}:${tag}`;
  void onDownload(full, "amd64", false);
}

function PullView({
  image,
  view,
  arch,
  onRetry,
  onBack,
}: {
  image: string;
  view: PullView;
  arch: string;
  onRetry: () => void;
  onBack: () => void;
}) {
  return (
    <div className="rail-full">
      <div className="page-head">
        <button className="ghost" type="button" onClick={onBack}>
          ← 返回
        </button>
        <span className="page-title mono">
          {image} {arch ? `(${arch})` : ""}
        </span>
      </div>
      {view.kind === "running" && <ProgressView progress={view.progress} log={view.log} />}
      {view.kind === "error" && (
        <div className="state state-error">
          <p className="state-title">拉取失败</p>
          <p className="error-message">{view.message}</p>
          <details className="raw-error">
            <summary>查看原始错误</summary>
            <pre className="error-text">{view.raw}</pre>
          </details>
          <div className="state-actions">
            <button className="ghost" type="button" onClick={onRetry}>
              重新拉取
            </button>
          </div>
        </div>
      )}
      {view.kind === "result" && <ResultCard data={view.data} />}
    </div>
  );
}

function ResultCard({ data }: { data: PullResult }) {
  async function copyCmd() {
    await navigator.clipboard.writeText(`docker load -i ${data.tar_path}`);
  }
  return (
    <div className="result">
      <div className="result-head">
        <span className="ok-dot" aria-hidden="true" />
        <span className="result-title">拉取完成</span>
      </div>
      <dl className="kv">
        <dt>镜像</dt>
        <dd className="mono">{data.image}</dd>
        <dt>镜像 ID</dt>
        <dd className="mono">{data.top_id}</dd>
        <dt>层数</dt>
        <dd className="mono">{data.layer_count}</dd>
        <dt>输出</dt>
        <dd className="mono">{data.tar_path}</dd>
      </dl>
      <div className="cmd-line">
        <code className="mono cmd-text">docker load -i {data.tar_path}</code>
        <button className="ghost copy" type="button" onClick={copyCmd}>
          复制命令
        </button>
      </div>
      <p className="hint">之后用 <code>docker load -i …</code> 即可导入该 tar。</p>
    </div>
  );
}

function ProgressView({ progress, log }: { progress: ProgressPayload; log: string[] }) {
  const pct = progress.total > 0 ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : 0;
  return (
    <div className="progress">
      <div className="progress-head">
        <span className="spinner" aria-hidden="true" />
        <span className="progress-stage mono">{progress.name}</span>
      </div>
      <div className="bar">
        <div className="bar-fill" style={{ width: `${pct}%` }} />
      </div>
      <p className="progress-message">{progress.message}</p>
      {log.length > 0 && (
        <ul className="log">
          {log.map((line, i) => (
            <li key={i}>{line}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

/* ================= 设置抽屉（settings） ================= */

interface SettingsProps {
  open: boolean;
  onClose: () => void;
  registries: RegistryCfg[];
  setRegistries: (r: RegistryCfg[]) => void;
  accounts: AccountCfg[];
  setAccounts: (a: AccountCfg[]) => void;
  downloadDir: string;
  setDownloadDir: (d: string) => void;
}

function SettingsDrawer({ open, onClose, registries, setRegistries, accounts, setAccounts, downloadDir, setDownloadDir }: SettingsProps) {
  const [newReg, setNewReg] = useState({ name: "", host: "", searchUrl: "" });
  const [newAcc, setNewAcc] = useState({ registry: "", username: "", password: "" });

  if (!open) return null;

  /** 打开目录选择对话框并填入选中目录。 */
  async function useDefaultDir() {
    try {
      const dir = await openDialog({
        directory: true,
        multiple: false,
        title: "选择下载目录",
      });
      if (typeof dir === "string" && dir) setDownloadDir(dir);
    } catch {
      /* 用户取消或出错，保持现状 */
    }
  }

  function addRegistry() {
    if (!newReg.host.trim()) return;
    setRegistries([
      ...registries,
      { id: uid(), name: newReg.name.trim(), host: newReg.host.trim(), searchUrl: newReg.searchUrl.trim() },
    ]);
    setNewReg({ name: "", host: "", searchUrl: "" });
  }
  function addAccount() {
    if (!newAcc.registry.trim() || !newAcc.username.trim()) return;
    setAccounts([...accounts, { registry: newAcc.registry.trim(), username: newAcc.username.trim(), password: newAcc.password }]);
    setNewAcc({ registry: "", username: "", password: "" });
  }

  return (
    <div className="drawer-mask" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()} aria-label="设置">
        <div className="drawer-head">
          <h2 className="drawer-title">设置</h2>
          <button className="ghost" type="button" onClick={onClose}>
            关闭
          </button>
        </div>

        <section className="settings-sec">
          <h3 className="settings-h3">下载目录</h3>
          <p className="settings-hint">拉取的 tar 文件保存位置。留空时使用当前目录。</p>
          <div className="settings-form dir-form">
            <input
              className="mono grow"
              value={downloadDir}
              onChange={(e) => setDownloadDir(e.currentTarget.value)}
              spellCheck={false}
              placeholder="例如 C:\Users\me\Downloads"
            />
            <button className="ghost" type="button" onClick={useDefaultDir} title="选择目录存放拉取的 tar">
              选择目录…
            </button>
          </div>
        </section>

        <section className="settings-sec">
          <h3 className="settings-h3">自定义镜像库</h3>
          <p className="settings-hint">补充预置以外的私有/第三方镜像库，配置后会出现在主界面「镜像库来源」下拉。</p>

          {registries.length === 0 && <p className="search-empty">暂无自定义镜像库。常用镜像库（ghcr.io/gcr.io/…）已直接预置到搜索来源下拉。</p>}
          <ul className="settings-list">
            {registries.map((r) => (
              <li key={r.id} className="settings-item">
                <div>
                  <span className="mono">{r.name || r.host}</span>
                  {r.searchUrl && <span className="settings-item-note mono">{r.searchUrl}</span>}
                </div>
                <button
                  className="ghost danger"
                  type="button"
                  onClick={() => setRegistries(registries.filter((x) => x.id !== r.id))}
                >
                  删除
                </button>
              </li>
            ))}
          </ul>
          <div className="settings-form">
            <input className="mono" placeholder="名称（展示用）" value={newReg.name} onChange={(e) => setNewReg({ ...newReg, name: e.currentTarget.value })} />
            <input className="mono" placeholder="主机，如 my-registry.com" value={newReg.host} onChange={(e) => setNewReg({ ...newReg, host: e.currentTarget.value })} />
            <input className="mono" placeholder="搜索地址（可选）" value={newReg.searchUrl} onChange={(e) => setNewReg({ ...newReg, searchUrl: e.currentTarget.value })} />
            <button className="ghost" type="button" onClick={addRegistry}>
              添加
            </button>
          </div>
        </section>

        <section className="settings-sec">
          <h3 className="settings-h3">私有账号</h3>
          <p className="settings-hint">按 registry 主机保存用户名/密码，仅存于本机 localStorage，拉取/检查私有镜像时自动使用。</p>
          {accounts.length === 0 && <p className="search-empty">尚未配置账号。</p>}
          <ul className="settings-list">
            {accounts.map((a, i) => (
              <li key={i} className="settings-item">
                <div>
                  <span className="mono">{a.registry}</span>
                  <span className="settings-item-note mono">@{a.username}</span>
                </div>
                <button className="ghost danger" type="button" onClick={() => setAccounts(accounts.filter((_, j) => j !== i))}>
                  删除
                </button>
              </li>
            ))}
          </ul>
          <div className="settings-form">
            <input className="mono" placeholder="registry 主机" value={newAcc.registry} onChange={(e) => setNewAcc({ ...newAcc, registry: e.currentTarget.value })} />
            <input className="mono" placeholder="用户名" value={newAcc.username} onChange={(e) => setNewAcc({ ...newAcc, username: e.currentTarget.value })} />
            <input className="mono" type="password" placeholder="密码" value={newAcc.password} onChange={(e) => setNewAcc({ ...newAcc, password: e.currentTarget.value })} />
            <button className="ghost" type="button" onClick={addAccount}>
              添加
            </button>
          </div>
        </section>
      </aside>
    </div>
  );
}

/* ===== 搜索来源选择（options / resolve） ===== */

/** 合并预置 + 用户自定义，给「搜索来源」下拉生成可选项。 */
function sourceOptions(userRegs: RegistryCfg[]): Array<{ id: string; label: string; searchUrl?: string; host: string }> {
  const builtin = REGISTRY_PRESETS.map((p) => ({
    id: `preset:${p.host}`,
    label: p.name,
    searchUrl: p.searchUrl || undefined,
    host: p.host,
  }));
  const user = userRegs.map((r) => ({
    id: r.id,
    label: r.name || r.host,
    searchUrl: r.searchUrl || undefined,
    host: r.host,
  }));
  return [{ id: "hub", label: "Docker Hub", searchUrl: undefined, host: HUB_HOST }, ...builtin, ...user];
}

function resolveSource(id: string, userRegs: RegistryCfg[]): { searchUrl: string | null; host: string } {
  const opts = sourceOptions(userRegs).find((o) => o.id === id);
  if (opts) {
    return { searchUrl: opts.searchUrl || null, host: opts.host };
  }
  return { searchUrl: null, host: HUB_HOST };
}

function hasHostPrefix(s: string): boolean {
  return /\//.test(s.trim());
}

function normalizedHost(h: string): string {
  return hostOf(h.includes("/") ? h : h + "/x");
}

function concreteArch(arch: Arch): string {
  return arch === "any" ? "amd64" : arch;
}

export default App;