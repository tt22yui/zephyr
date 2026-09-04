//! 本地发布日志脚本：把 CHANGELOG.md 顶部的 `[Unreleased]` 段版本化为当前版本。
//!
//! 用法：`npm run changelog:release`
//! 行为（幂等）：若当前版本条目已存在则跳过；否则将 Unreleased 段标题改为
//! `[<version>] - <今天>`（纯占位注释会被移除），并在文件顶部重建一个空 Unreleased 段。
//!
//! 变更要点遵循 Keep a Changelog 约定，由开发者在 Unreleased 段手动维护；本脚本
//! 只负责在发布时完成「版本化」动作，不臆造变更内容。

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const changelogPath = join(root, "CHANGELOG.md");
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const version = pkg.version;
const today = new Date().toISOString().slice(0, 10);

if (!existsSync(changelogPath)) {
  console.error(`未找到 ${changelogPath}，请先创建后再发布。`);
  process.exit(1);
}

// 幂等：当前版本条目已存在则不做任何改动。
if (new RegExp(`^## \\[${escapeRegExp(version)}\\]`, "m").test(readFileSync(changelogPath, "utf8"))) {
  console.log(`CHANGELOG 已存在 [${version}] 条目，跳过。`);
  process.exit(0);
}

let content = readFileSync(changelogPath, "utf8");

// 匹配整个 Unreleased 段（直到下一个 ## 或文件结尾）。
const unreleased = /## \[Unreleased\]\n([\s\S]*?)(?=\n## |\n# |$)/;
const m = content.match(unreleased);
if (!m) {
  console.error("CHANGELOG 顶层未找到 `## [Unreleased]` 段，跳过。");
  process.exit(1);
}

// 去掉纯占位注释行（如 `<!-- ... -->`），避免残留进已发布条目。
const body = m[1]
  .split("\n")
  .filter((line) => !/^\s*<!--[\s\S]*?-->\s*$/.test(line))
  .join("\n")
  .replace(/\n{3,}/g, "\n\n")
  .trim();

const releaseSeg = `## [${version}] - ${today}${body ? `\n\n${body}` : ""}`;
content = content.replace(m[0], releaseSeg);

// 在文件顶部（标题之下）重建一个空 Unreleased 段。
const headEnd = content.search(/\n## \[/);
if (headEnd === -1) {
  content += `\n\n## [Unreleased]\n\n<!-- 下一版本的变更要点，发布时运行 \`npm run changelog:release\` -->\n`;
} else {
  content =
    content.slice(0, headEnd) +
    `\n\n## [Unreleased]\n\n<!-- 下一版本的变更要点，发布时运行 \`npm run changelog:release\` -->\n` +
    content.slice(headEnd);
}

writeFileSync(changelogPath, content, "utf8");
console.log(`已将 Unreleased 版本化为 [${version}] - ${today}，并新建空 Unreleased 段。`);

function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}