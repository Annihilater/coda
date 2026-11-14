"""Coda 四分类发版预检与顺序提交推送（commit + push per category）动态工作流。

支持 Args：
- target_version (str, 可选): 目标语义版本号，例如 "0.3.9"（自动去除前导 "v"）。
- dry_run (bool, 可选, 默认 True): True 仅做并行只读预检与计划生成；False 才执行逐分类改号、自检、提交与推送。
"""

import json
import re


PREFLIGHT_SCHEMA = {
    "type": "object",
    "properties": {
        "category": {"type": "string"},
        "files": {"type": "array", "items": {"type": "string"}},
        "current_version": {"type": "string"},
        "dirty_files": {"type": "array", "items": {"type": "string"}},
        "ready": {"type": "boolean"},
    },
    "required": ["category", "files", "current_version", "dirty_files", "ready"],
}

BUMP_RESULT_SCHEMA = {
    "type": "object",
    "properties": {
        "category": {"type": "string"},
        "commit_sha": {"type": "string"},
        "pushed": {"type": "boolean"},
        "verification": {"type": "string"},
    },
    "required": ["category", "commit_sha", "pushed", "verification"],
}

CATEGORIES = [
    {
        "category": "workspace",
        "files": ["Cargo.toml", "Cargo.lock"],
        "commit_template": "chore(workspace): bump rust workspace version to {ver}",
        "instruction": (
            "仅修改 Cargo.toml 的 [workspace.package] version 为 \"{ver}\"，"
            "然后执行 export PATH=\"/opt/homebrew/bin:$HOME/.local/bin:$PATH\" && cargo check --workspace "
            "同步更新 Cargo.lock 中的工作区 crate 版本（不要手动改动外部依赖 dirs-sys 0.3.7）。"
        ),
    },
    {
        "category": "apple",
        "files": ["apple/project.yml"],
        "commit_template": "chore(apple): bump mobile marketing version to {ver}",
        "instruction": (
            "仅修改 apple/project.yml 中的 MARKETING_VERSION 为 \"{ver}\"，不改其他配置或 Swift 代码。"
        ),
    },
    {
        "category": "web",
        "files": ["web/src/app.ts"],
        "commit_template": "chore(web): bump fallback display version to v{ver}",
        "instruction": (
            "仅修改 web/src/app.ts 中服务列表展示的默认版本字符串与 siteInfo fallback 版本为 \"v{ver}\"，"
            "并执行 export PATH=\"/opt/homebrew/bin:$HOME/.local/bin:$HOME/.bun/bin:$PATH\" && "
            "(cd web && bun run build) 验证前端构建通过。"
        ),
    },
    {
        "category": "release",
        "files": [".github/workflows/release.yml", "deploy/.env.example"],
        "commit_template": "chore(release): bump release workflow and env template to v{ver}",
        "instruction": (
            "仅修改 .github/workflows/release.yml 中 workflow_dispatch.inputs.tag_name 的 description 与 default 为 v{ver}，"
            "以及 deploy/.env.example 中的 # CODA_RELEASE_VERSION=v{ver}。"
        ),
    },
]


cfg = globals().get("args") or {}
target_version = str(cfg.get("target_version") or "").lstrip("v").strip()
dry_run = bool(cfg.get("dry_run", True))

phase("1. 四分类并行版本与工作区预检")


def inspect_category(spec):
    files_str = " ".join(spec["files"])
    return agent(
        prompt=(
            "你处于只读预检模式，严禁修改任何文件。\n"
            f"请检查发版分类 `{spec['category']}` 对应的文件：{spec['files']}\n"
            f"1. 运行 `git status --porcelain -- {files_str}` 检查是否有未提交修改（列入 dirty_files）。\n"
            "2. 读取对应文件中当前记录的 Coda 版本号（去掉前导 v 后填为标准语义版本，例如 0.3.8）。\n"
            "3. 若所有文件存在、可明确解析出单一 current_version 且 dirty_files 为空，则 ready=true。\n"
            "完成后调用 finish 返回结构化 JSON。"
        ),
        schema=PREFLIGHT_SCHEMA,
        role=f"Preflight · {spec['category']}",
        model="flash",
    )


preflight_reports = await parallel(CATEGORIES, inspect_category)
distinct_versions = sorted({r["current_version"] for r in preflight_reports})
all_dirty = [
    {"category": r["category"], "dirty_files": r["dirty_files"]}
    for r in preflight_reports
    if r["dirty_files"]
]

for r in preflight_reports:
    log(
        f"[{r['category']}] 当前版本={r['current_version']}, "
        f"dirty={len(r['dirty_files'])}, ready={r['ready']}"
    )

version_aligned = len(distinct_versions) == 1
if not version_aligned:
    log(f"警告：各分类当前版本不一致: {distinct_versions}")

phase("2. 确定性顺序流水线编排")
if dry_run or not target_version:
    mode = "dry_run" if dry_run else "missing_target_version"
    planned_ver = target_version or "<next-version>"
    plan_steps = [
        {
            "step": idx + 1,
            "category": spec["category"],
            "files": spec["files"],
            "commit_message": spec["commit_template"].format(ver=planned_ver),
            "push_command": "git push origin main",
        }
        for idx, spec in enumerate(CATEGORIES)
    ]
    log(
        f"处于只读模式 ({mode})，未执行任何写操作；"
        f"四端当前版本是否统一={version_aligned} ({distinct_versions})。"
    )
    print(
        json.dumps(
            {
                "mode": mode,
                "version_aligned": version_aligned,
                "distinct_versions": distinct_versions,
                "dirty_categories": all_dirty,
                "preflight_reports": preflight_reports,
                "planned_pipeline": plan_steps,
                "next_tag_command": f'git tag -a v{planned_ver} -m "Coda v{planned_ver}" && git push origin v{planned_ver}',
            },
            ensure_ascii=False,
            indent=2,
        )
    )
else:
    if not re.match(r"^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$", target_version):
        raise ValueError(f"非法版本号格式: {target_version}")
    if all_dirty or any(not r["ready"] for r in preflight_reports):
        raise RuntimeError(
            f"工作区未就绪或存在未提交改动，中止发版流水线: dirty={all_dirty}"
        )

    def run_category_bump(spec):
        msg = spec["commit_template"].format(ver=target_version)
        files_str = " ".join(spec["files"])
        detail = spec["instruction"].format(ver=target_version)
        return agent(
            prompt=(
                f"执行发版分类 `{spec['category']}` 的版本升级、验证、提交与推送：\n"
                f"1. {detail}\n"
                f"2. 执行 `git add {files_str} && git commit -m \"{msg}\"`\n"
                "3. 立即执行 `git push origin main`\n"
                "4. 获取新提交的短 SHA（`git rev-parse --short HEAD`），调用 finish 返回结果。"
            ),
            schema=BUMP_RESULT_SCHEMA,
            role=f"Bump & Push · {spec['category']}",
            model="flash",
        )

    bump_results = await pipeline(CATEGORIES, run_category_bump)
    for res in bump_results:
        log(f"已提交并推送 [{res['category']}]: {res['commit_sha']} (pushed={res['pushed']})")

    output = {
        "mode": "executed",
        "target_version": target_version,
        "previous_versions": distinct_versions,
        "commits": bump_results,
        "recommended_tag_command": (
            f'git tag -a v{target_version} -m "Coda v{target_version}" '
            f"&& git push origin v{target_version}"
        ),
    }
    log(f"四分类顺次提交推送全部完成，下一步可执行打标命令发布 v{target_version}")
    print(json.dumps(output, ensure_ascii=False, indent=2))
