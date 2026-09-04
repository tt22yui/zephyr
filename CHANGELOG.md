# Changelog

本文件记录「泽帆 Zephyr」的重要变更，遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/) 的版本化与分类约定。

## \[Unreleased]

<!-- 为下一版本的发布收集变更要点；发布时运行 `npm run changelog:release`，本段标题会自动换成当前版本号，并生成一个全新的空 Unreleased 段。 -->

## \[0.1.4] - 2026-09-04

### Added

- 下载列表重设计：仅保留镜像名 + 状态，运行中显示实时进度条。

- 新增「停止」：排队任务直接取消；运行中任务经后端取消令牌真正中断。

- 后端 blob 流式读取，下载出错或停止时清理残留 tar。

- 「清空已完成」移至抽屉头部与关闭同行。

- 详情页压缩间距更紧凑。

