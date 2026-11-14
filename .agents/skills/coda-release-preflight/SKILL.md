---
name: coda-release-preflight
description: >-
  当用户准备为 coda 仓库发布新版本，需要按 Rust Workspace、Apple、Web、Release
  四个分类依次检查或更新版本号并执行“每次提交即推送 origin/main”的确定性发版流水线时使用此技能。
---

# Coda 分类发版预检与流水线 (`coda-release-preflight`)

本技能使用 `run_workflow` 编排可重复执行的分类发版控制流脚本：[release_preflight_workflow.py](./scripts/release_preflight_workflow.py)。

工作流默认以安全只读模式（`dry_run=true`）运行：
1. **阶段 1（四分类并行预检）**：使用 `parallel()` 并发核对 `workspace`、`apple`、`web`、`release` 四个分类的版本号一致性与工作区脏文件状态。
2. **阶段 2（确定性顺序流水线）**：
   - 当 `dry_run=true`（默认）或未提供 `target_version` 时，仅生成并打印确定的四步提交与打标计划，不改动任何文件、不执行提交或推送；
   - 当用户明确审阅后要求实际执行（`dry_run=false` 且传入 `target_version`，如 `"0.3.9"`）时，先由 Python 确定性校验全工作区干净，再通过 `pipeline()` 严格按顺序逐个分类更新、自检、`git commit` 并 `git push origin main`。

## 执行步骤

1. 先审阅脚本内容：[release_preflight_workflow.py](./scripts/release_preflight_workflow.py)。
2. 调用 `run_workflow` 运行只读预检（推荐先运行一次预检）：
   - `ScriptPath`: `<workspace_root>/.agents/skills/coda-release-preflight/scripts/release_preflight_workflow.py`
   - `Description`: `"Coda 四分类发版预检与顺序提交流水线"`
   - `Args`：
     - `target_version` (`string`，可选，例如 `"0.3.9"`）：目标语义版本号（不含 `v` 前缀）。
     - `dry_run` (`boolean`，可选，默认 `true`)：`true` 为只读预检规划；仅在明确执行真实发版改号时传 `false`。
3. 如为真实发版（`dry_run=false`），流水分步完成后再根据输出建议手动或确认打标推送：`git tag -a v<target_version> -m "Coda v<target_version>" && git push origin v<target_version>`。
