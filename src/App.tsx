import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

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

interface SearchResult {
  name: string;
  description: string;
  stars: number;
  is_official: boolean;
  updated_at: string;
}

type ViewState =
  | { kind: "idle" }
  | { kind: "running"; progress: ProgressPayload; log: string[] }
  | { kind: "error"; raw: string; message: string }
  | { kind: "result"; data: PullResult };

const EXAMPLES = [
  "nginx:latest",
  "nginx:1.25",
  "ghcr.io/home-assistant/home-assistant:stable",
  "docker.io/motrixapp/motrix-server:2.0.0-beta.27",
];

const STORE = {
  arch: "dpt.arch",
  http: "dpt.http",
  history: "dpt.history",
  search: "dpt.search",
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
  } catch { /* ignore */ }
}

function friendlyError(raw: string): string {
  if (/404/.test(raw)) {
    return "镜像或标签不存在（404）。该仓库可能只发布了特定标签，latest 不一定存在，请换一个存在的 tag 重试。";
  }
  if (/JSON/.test(raw) || /解析/.test(raw)) {
    return "无法解析 registry 返回的数据。可能是仓库不存在、或需要认证。请检查镜像名是否正确，私有仓库请填写用户名/密码。";
  }
  if (/连接|timed out|timeout|tls|证书|certificate/i.test(raw)) {
    return "网络连接失败。请确认网络/代理可用（本地 registry 可勾选“使用 HTTP”），然后重试。";
  }
  if (/manifest|清单|架构|architecture/.test(raw)) {
    return "清单或架构不匹配。请检查镜像是否支持你选择的目标架构。";
  }
  return raw;
}

function App() {
  const [image, setImage] = useState("");
  const [arch, setArch] = useState<string>(() => load(STORE.arch, "amd64"));
  const [useHttp, setUseHttp] = useState<boolean>(() => load(STORE.http, false));
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [outFile, setOutFile] = useState("");
  const [history, setHistory] = useState<string[]>(() => load(STORE.history, []));
  const [searchQuery, setSearchQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [searchResults, setSearchResults] = useState<SearchResult[] | null>(null);
  const [searchError, setSearchError] = useState("");
  const [searchHistory, setSearchHistory] = useState<string[]>(() => load(STORE.search, []));
  const [busy, setBusy] = useState(false);
  const [view, setView] = useState<ViewState>({ kind: "idle" });

  // 记住设置
  useEffect(() => save(STORE.arch, arch), [arch]);
  useEffect(() => save(STORE.http, useHttp), [useHttp]);

  // 订阅进度事件
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listen<ProgressPayload>('pull://progress', (ev) => {
      setView((prev) =>
        prev.kind === "running"
          ? { ...prev, progress: ev.payload, log: [...prev.log.slice(-29), ev.payload.message] }
          : prev,
      );
    }).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  const runPull = useCallback(async (img: string) => {
    setBusy(true);
    setView({ kind: "running", progress: { name: "auth", done: 0, total: 1, message: "准备开始…" }, log: ["准备开始…"] });
    try {
      const r = await invoke<PullResult>("pull_image", {
        image: img,
        arch: arch || null,
        useHttp: useHttp || null,
        username: username || null,
        password: password || null,
        outFile: outFile || null,
      });
      const next = [...history.filter((h) => h !== img), img].slice(-8);
      setHistory(next);
      save(STORE.history, next);
      setView({ kind: "result", data: r });
    } catch (err) {
      const raw = String(err);
      setView({ kind: "error", raw, message: friendlyError(raw) });
    } finally {
      setBusy(false);
    }
  }, [arch, useHttp, username, password, outFile, history]);

  function doPull(e: React.FormEvent) {
    e.preventDefault();
    if (!image.trim() || busy) return;
    void runPull(image.trim());
  }

  const searchImage = useCallback(
    async (q: string) => {
      const kw = q.trim();
      if (!kw || busy) return;
      setSearching(true);
      setSearchError("");
      setSearchResults(null);
      try {
        const results = await invoke<SearchResult[]>("search_image", { query: kw, pageSize: 25 });
        setSearchResults(results);
        if (results.length > 0) {
          const next = [...searchHistory.filter((h) => h !== kw), kw].slice(-8);
          setSearchHistory(next);
          save(STORE.search, next);
        }
      } catch (err) {
        setSearchError(friendlyError(String(err)));
      } finally {
        setSearching(false);
      }
    },
    [busy, searchHistory],
  );

  function doSearch(e: React.FormEvent) {
    e.preventDefault();
    void searchImage(searchQuery);
  }

  function pickResult(name: string) {
    setImage(name);
    setSearchQuery("");
    setSearchResults(null);
    setSearchError("");
  }

  async function copyCmd(path: string) {
    await navigator.clipboard.writeText(`docker load -i ${path}`);
  }

  const isEmpty = !image.trim();

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="brand-dot" aria-hidden="true" />
          <span className="brand-name">泽帆 Zephyr</span>
          <span className="brand-sub">registry&nbsp;→&nbsp;tar · no daemon</span>
        </div>
        {busy && (
          <div className="busy">
            <span className="spinner" aria-hidden="true" />
            {view.kind === "running" ? view.progress.message : "拉取中…"}
          </div>
        )}
      </header>

      <div className="layout">
        <section className="panel form-panel" aria-label="拉取配置">
          <div className="search" aria-label="镜像搜索">
            <form className="search-bar" onSubmit={doSearch}>
              <input
                className="mono"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.currentTarget.value)}
                list="search-history"
                spellCheck={false}
                placeholder="搜索 Docker Hub 镜像，如 nginx"
              />
              <datalist id="search-history">
                {searchHistory.map((h) => <option key={h} value={h} />)}
              </datalist>
              <button className="ghost" type="submit" disabled={busy || searching || !searchQuery.trim()}>
                {searching ? "搜索中…" : "搜索"}
              </button>
            </form>

            {searching && (
              <div className="search-status">
                <span className="spinner" aria-hidden="true" />
                搜索中…
              </div>
            )}
            {!searching && searchError && <p className="search-error">{searchError}</p>}
            {!searching && !searchError && searchResults !== null && (
              searchResults.length === 0 ? (
                <p className="search-empty">未找到相关镜像，换个关键词试试。</p>
              ) : (
                <ul className="search-results">
                  {searchResults.map((r) => (
                    <li key={r.name}>
                      <button
                        type="button"
                        className="search-item"
                        onClick={() => pickResult(r.name)}
                        title={`填入 ${r.name}`}
                      >
                        <span className="search-item-name mono">{r.name}</span>
                        {r.is_official && <span className="badge-official">官方</span>}
                        <span className="search-item-stars">⭐ {r.stars}</span>
                        <span className="search-item-desc">{r.description || "（无简介）"}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )
            )}
          </div>

          <form onSubmit={doPull}>
            <label className="field">
              <span className="field-label">镜像名称</span>
              <input
                className="mono"
                value={image}
                onChange={(e) => setImage(e.currentTarget.value)}
                list="img-history"
                spellCheck={false}
                placeholder="如 nginx:latest"
                autoFocus
              />
              <datalist id="img-history">
                {history.map((h) => <option key={h} value={h} />)}
              </datalist>
              <span className="field-hint">
                支持 <code>name:tag</code>、<code>host/name:tag</code>、<code>name@sha256:…</code>
              </span>
            </label>

            <div className="examples" aria-label="快速示例">
              {EXAMPLES.map((ex) => (
                <button key={ex} type="button" className="chip" onClick={() => setImage(ex)} title={`填入 ${ex}`}>
                  {ex}
                </button>
              ))}
            </div>

            <div className="field-row">
              <label className="field grow">
                <span className="field-label">目标架构</span>
                <select className="mono" value={arch} onChange={(e) => setArch(e.currentTarget.value)}>
                  <option value="amd64">amd64</option>
                  <option value="arm64">arm64</option>
                </select>
              </label>
              <label className="toggle">
                <input type="checkbox" checked={useHttp} onChange={(e) => setUseHttp(e.currentTarget.checked)} />
                <span className="toggle-track" aria-hidden="true" />
                <span className="toggle-label">使用 HTTP</span>
              </label>
            </div>

            <details className="auth">
              <summary>认证（私有仓库可选）</summary>
              <div className="field-row">
                <label className="field grow">
                  <span className="field-label">用户名</span>
                  <input className="mono" value={username} onChange={(e) => setUsername(e.currentTarget.value)} autoComplete="off" spellCheck={false} />
                </label>
                <label className="field grow">
                  <span className="field-label">密码</span>
                  <input className="mono" type="password" value={password} onChange={(e) => setPassword(e.currentTarget.value)} />
                </label>
              </div>
            </details>

            <label className="field">
              <span className="field-label">输出文件</span>
              <input
                className="mono"
                value={outFile}
                onChange={(e) => setOutFile(e.currentTarget.value)}
                spellCheck={false}
                placeholder="留空默认 ./<仓库>_<tag>.tar"
              />
            </label>

            <button className="primary" type="submit" disabled={busy || isEmpty}>
              {busy ? "拉取中…" : isEmpty ? "请填写镜像名" : "拉取镜像"}
            </button>
          </form>
        </section>

        <section className="panel rail" aria-live="polite">
          {view.kind === "idle" && (
            <div className="state">
              <div className="state-icon" aria-hidden="true">⇣</div>
              <p className="state-title">等待任务</p>
              <p className="state-desc">填写镜像名称，点击「拉取镜像」开始。可从上方示例或输入历史快速选择。</p>
            </div>
          )}

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
                <button className="ghost" type="button" onClick={() => { void runPull(image); }}>重新拉取</button>
              </div>
            </div>
          )}

          {view.kind === "result" && (
            <div className="result">
              <div className="result-head">
                <span className="ok-dot" aria-hidden="true" />
                <span className="result-title">拉取完成</span>
              </div>
              <dl className="kv">
                <dt>镜像</dt>
                <dd className="mono">{view.data.image}</dd>
                <dt>镜像 ID</dt>
                <dd className="mono">{view.data.top_id}</dd>
                <dt>层数</dt>
                <dd className="mono">{view.data.layer_count}</dd>
                <dt>输出</dt>
                <dd className="mono">{view.data.tar_path}</dd>
              </dl>
              <div className="cmd-line">
                <code className="mono cmd-text">docker load -i {view.data.tar_path}</code>
                <button className="ghost copy" type="button" onClick={() => copyCmd(view.data.tar_path)}>复制命令</button>
              </div>
              <p className="hint">之后用 <code>docker load -i …</code> 即可导入该 tar。</p>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function ProgressView({ progress, log }: { progress: ProgressPayload; log: string[] }) {
  const pct =
    progress.total > 0 ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : 0;
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
      <ul className="log">
        {log.map((line, i) => (
          <li key={i}>{line}</li>
        ))}
      </ul>
    </div>
  );
}

export default App;