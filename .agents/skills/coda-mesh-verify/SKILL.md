---
name: coda-mesh-verify
description: >-
  当用户需要在真实多端环境中端到端验证 Coda 的本地 herdr-core 终端托管、云端 WebSocket
  实时账户推送、macOS GPUI 桌面端与 Apple Swift 6 移动端对 Mac 本机及 openEuler、Debian、
  Ubuntu、CentOS Linux 服务器的并行装机入网与全双工控制能力时使用此技能。通过 Antigravity
  动态工作流确定性编排子智能体并保证异常安全的环境回收。
---

# Coda 全平台实时 Herdr 网格与 OrbStack 四发行版动态工作流 (`coda-mesh-verify`)

本技能使用 `run_workflow` 编排一段可审查、可重复执行的 Python 动态工作流脚本：[mesh_verify_workflow.py](./scripts/mesh_verify_workflow.py)，通过**确定性控制流**与**结构化交接**消除人工步骤遗漏与**提示漂移**，在完全隔离于 `~/.config/coda/daemon.json` 的环境中完成五阶段全链路实测：

1. **阶段 1（双架构并行构建）**：使用 `parallel()` 并发拉起两个构建**子智能体**：
   - macOS 本机构建：`coda-server`、`coda-daemon`、`coda-desktop` 以及 Apple Swift 6 客户端（`CodaSelfTest`）；
   - OrbStack `centos:9-Stream` 构建机：编译兼容各大发行版的 Linux `aarch64` 版 `coda-daemon` 并安装 Linux `herdr-core` (`hc`)，导出至 `~/.cache/coda-verify-mesh/`。
2. **阶段 2（隔离控制面与本机 Mac 混合宿主机拉起）**：启动测试专用的 `coda-server`（PostgreSQL `coda_test`）与隔离的本机 `hybrid` 模式宿主机 `macos-local-host`，并通过 JSON Schema 将服务地址、账号凭证与设备名称结构化交接给后续阶段。
3. **阶段 3（OrbStack 四大 Linux 发行版并行装机入网）**：通过 `parallel()` 同时编排四个**子智能体**，分别在 `openEuler 24.03`、`Debian 12`、`Ubuntu 24.04`、`CentOS 9 Stream` 虚拟机中创建测试 Git 工作区并执行 `deploy/install-daemon.sh --non-interactive` 自动化装机与登录入网。
4. **阶段 4（Rust 桌面端与 Apple Swift 6 移动端并行全链路实测）**：确认控制面已聚合全部 `1 + N` 台在线宿主机后，通过 `parallel()` 并发运行：
   - Rust 桌面验证器（`coda-daemon verify-mesh`）：校验 `/api/v1/events/ws`、`ListWorkspaces`、`GitDiffRequest`、`LaunchAgent`、`PtyAttach` / `PtyInput` / `PtyOutputChunk` 实时双向终端流；
   - Apple Swift 6 验证器（`CodaSelfTest` live mesh 模式）：从 Swift 原生网络栈与 E2EE 引擎并发接管全部 Mac 与 Linux 宿主机并完成真实终端读写与 Git Diff 校验。
5. **阶段 5（异常安全的确定性清理）**：无论前置阶段成功或失败，Python `try ... finally` 控制流均强制执行收尾**智能体**，停止临时进程、删除临时 OrbStack 虚拟机并清空 `~/.cache/coda-verify-mesh`，核验个人配置零污染、临时虚拟机零残留。

## 执行步骤

1. 先审阅工作流脚本：[mesh_verify_workflow.py](./scripts/mesh_verify_workflow.py)。
2. 调用 `run_workflow` 工具执行该脚本（可选传入 `Args`）：
   - `ScriptPath`: `<workspace_root>/.agents/skills/coda-mesh-verify/scripts/mesh_verify_workflow.py`
   - `Description`: `"Coda 全平台实时 Herdr 网格与 4 大 Linux 发行版 OrbStack 动态工作流实测"`
   - `Args`（可选 JSON 对象）：
     - `distros` (`array<string>`，可选，默认 `["openeuler:24.03", "debian:12", "ubuntu:24.04", "centos:9-Stream"]`)：要并行实测的 OrbStack Linux 发行版镜像列表。
     - `keep_vms` (`boolean`，可选，默认 `false`)：是否在工作流结束后保留临时 `coda-verify-*` 虚拟机用于人工排查（默认自动清理）。
3. 工作流完成后，审阅返回的 `Logs:` 与 `Output:` JSON 报告。
