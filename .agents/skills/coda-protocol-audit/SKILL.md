---
name: coda-protocol-audit
description: >-
  当用户需要在 coda 仓库中审查 E2EE 帧协议、RPC 契约在 Rust 服务端/守护进程、
  macOS GPUI 桌面端、Apple Swift 6 客户端与 Web 控制台四端的一致性，或验证无
  SwiftUI 宏泄漏与全端构建自检时使用此技能。通过 Antigravity 动态工作流并行编排子智能体并完成结构化交接。
---

# Coda 四端协议与架构动态工作流 (`coda-protocol-audit`)

本技能使用 `run_workflow` 编排一段可审查、可重复执行的 Python 动态工作流脚本：[protocol_audit_workflow.py](./scripts/protocol_audit_workflow.py)。

工作流通过三阶段实现**确定性控制流**与**结构化交接**：
1. **阶段 1（契约提取）**：单个只读智能体解析 `crates/coda-protocol/src/lib.rs` 与 `crates/coda-apple-ffi/src/lib.rs`，通过 JSON Schema 输出帧头标志位、RPC 变体与 FFI 导出清单。
2. **阶段 2（四端并行审查）**：通过 `parallel()` 并发拉起 4 个只读子智能体，分别核对 Rust Server/Daemon、macOS GPUI Desktop、Apple Swift 6（含 iPhone Duo `84pt` 系统栏安全区及 `@State`/`@Observable`/`@Bindable` 宏禁令）、Web Console。
3. **阶段 3（确定性门禁与汇总）**：当 `run_self_test` 为 `true` 时，自动执行 `cargo check --workspace`、`git grep` 宏禁令检查以及 `CodaSelfTest`，由 Python 控制流计算最终是否通过。

## 执行步骤

1. 先阅读并审查脚本内容：[protocol_audit_workflow.py](./scripts/protocol_audit_workflow.py)。
2. 调用 `run_workflow` 工具执行该脚本（可选传入 `Args`）：
   - `ScriptPath`: `<workspace_root>/.agents/skills/coda-protocol-audit/scripts/protocol_audit_workflow.py`
   - `Description`: `"Coda 四端协议契约对齐与编译/宏安全动态工作流巡检"`
   - `Args`（可选 JSON 对象）：
     - `focus` (`string`，默认 `""`)：本次额外重点关注的具体协议变更或界面模块（例如 `"uncompressed E2EE frame & iPhone Duo 84pt rail"`）。
     - `run_self_test` (`boolean`，默认 `true`)：是否在阶段 3 运行 `cargo check --workspace`、SwiftUI 宏排查与 `CodaSelfTest`。
3. 工作流完成后，审阅返回的 `Logs:` 与 `Output:` JSON 报告。
