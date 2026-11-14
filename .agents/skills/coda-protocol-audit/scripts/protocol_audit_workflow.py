"""Coda 四端协议契约对齐、SwiftUI 宏禁令与编译自检动态工作流。

可通过 Antigravity `run_workflow` 工具调用，支持以下 `Args`：
- focus (str, 可选): 本次审查重点关注的特性或协议字段。
- run_self_test (bool, 可选, 默认 True): 是否在阶段 3 执行本地编译与 CodaSelfTest 门禁。
"""

import json


CONTRACT_SCHEMA = {
    "type": "object",
    "properties": {
        "protocol_version": {"type": "string"},
        "frame_flags": {"type": "array", "items": {"type": "string"}},
        "rpc_variants": {"type": "array", "items": {"type": "string"}},
        "ffi_exports": {"type": "array", "items": {"type": "string"}},
    },
    "required": ["protocol_version", "frame_flags", "rpc_variants", "ffi_exports"],
}

AUDIT_SCHEMA = {
    "type": "object",
    "properties": {
        "stack": {"type": "string"},
        "aligned": {"type": "boolean"},
        "checked_points": {"type": "array", "items": {"type": "string"}},
        "issues": {"type": "array", "items": {"type": "string"}},
    },
    "required": ["stack", "aligned", "checked_points", "issues"],
}

GATE_SCHEMA = {
    "type": "object",
    "properties": {
        "cargo_check_ok": {"type": "boolean"},
        "macro_free_ok": {"type": "boolean"},
        "apple_selftest_ok": {"type": "boolean"},
        "details": {"type": "string"},
    },
    "required": ["cargo_check_ok", "macro_free_ok", "apple_selftest_ok", "details"],
}

AUDIT_TARGETS = [
    (
        "Rust Server & Daemon",
        "crates/coda-server/src/lib.rs、crates/coda-server/src/relay.rs 与 "
        "crates/coda-daemon/src/lib.rs：核对 E2EE 帧压缩/未压缩标志位处理、"
        "中继隧道转发与 PeerMessage RPC 分发是否与契约一致。",
    ),
    (
        "macOS GPUI Desktop",
        "crates/coda-desktop/src/lib.rs 与 crates/coda-desktop/src/pty.rs："
        "核对 Herdr PTY 会话桥接、OSC 7501 状态解析以及 SmartPeerClient RPC 调用。",
    ),
    (
        "Apple Swift 6 Client",
        "apple/Sources/CodaKit/CodaNetworkClient.swift、"
        "apple/Sources/CodaKit/CodaCoreModels.swift、"
        "apple/Sources/CodaUI/CodaAdaptiveViews.swift 与 "
        "apple/Sources/CodaUI/CodaIPhoneDuoViews.swift："
        "核对未压缩 E2EE 帧协商、pty_resize 单向 RPC、iPhone Duo 右侧 84pt 系统栏安全区，"
        "并确认 apple/Sources 与 apple/iOSApp 中不存在 @State/@Observable/@Bindable 宏。",
    ),
    (
        "Web Console",
        "web/src/app.ts：核对 WebSocket E2EE 握手、fzstd 压缩与未压缩帧兼容分支以及 RPC 消息处理。",
    ),
]


cfg = globals().get("args") or {}
focus = str(cfg.get("focus") or "").strip()
run_self_test = bool(cfg.get("run_self_test", True))
workspace_root = WORKSPACES[0] if WORKSPACES else "."
focus_note = f"\n本次额外重点关注：{focus}" if focus else ""

phase("1. 提取协议与 FFI 契约")
log(f"开始巡检工作区: {workspace_root}")
contract = await agent(
    prompt=(
        "你处于只读审查模式，严禁修改任何文件。\n"
        "请读取 crates/coda-protocol/src/lib.rs 与 crates/coda-apple-ffi/src/lib.rs，"
        "提取当前真实代码中的：\n"
        "1. protocol_version（转为字符串输出，例如帧头版本或常量说明）\n"
        "2. frame_flags（E2EE 帧头标志位与压缩/未压缩标志）\n"
        "3. rpc_variants（PeerMessage / RPC 核心变体与关键动作名）\n"
        "4. ffi_exports（crates/coda-apple-ffi/src/lib.rs 中的 #[no_mangle] pub extern \"C\" 导出函数名）\n"
        "完成后调用 finish 返回严格符合 Schema 的结构化 JSON。"
        f"{focus_note}"
    ),
    schema=CONTRACT_SCHEMA,
    role="Protocol Contract Extractor",
    model="flash",
)
log(
    f"契约提取完成: 版本={contract['protocol_version']}, "
    f"标志位={len(contract['frame_flags'])} 项, "
    f"RPC={len(contract['rpc_variants'])} 项, "
    f"FFI={len(contract['ffi_exports'])} 项"
)

phase("2. 四端并行契约与架构审查")
contract_json = json.dumps(contract, ensure_ascii=False, indent=2)


def build_audit_task(item):
    stack, target_scope = item
    return agent(
        prompt=(
            "你处于只读审查模式，严禁修改任何文件，也不要运行破坏性命令。\n"
            f"审查对象：{stack}\n"
            f"检查范围：{target_scope}{focus_note}\n\n"
            "以下是阶段 1 提取的权威协议与 FFI 契约（结构化交接数据）：\n"
            f"{contract_json}\n\n"
            "请结合检索（grep/rg）与定向文件读取核对上述源码实现：\n"
            "- stack: 必须原样填写为 \"" + stack + "\"\n"
            "- aligned: 仅当核心契约与约束完全一致时为 true\n"
            "- checked_points: 列出你实际查验过的文件/符号与结论（简明条目）\n"
            "- issues: 若发现不一致或隐患，写明精确的文件路径、符号名及问题描述；若无则返回空数组 []\n"
            "完成后调用 finish 提交结构化 JSON。"
        ),
        schema=AUDIT_SCHEMA,
        role=f"Audit · {stack}",
        model="flash",
    )


reports = await parallel(AUDIT_TARGETS, build_audit_task)
for report in reports:
    status = "对齐" if report["aligned"] else f"发现 {len(report['issues'])} 处差异"
    log(f"[{report['stack']}] {status}")

phase("3. 确定性门禁与自检汇总")
gate = None
if run_self_test:
    gate = await agent(
        prompt=(
            "你处于只读验证模式，严禁编辑任何源码文件。\n"
            "请依次运行以下验证命令（每次命令均需先执行 "
            "export PATH=\"/opt/homebrew/bin:$HOME/.local/bin:$PATH\"）：\n"
            "1. 宏禁令检查：git grep -nE '@State\\b|@Observable\\b|@Bindable\\b' "
            "apple/Sources apple/iOSApp || true （输出为空则 macro_free_ok = true）\n"
            "2. Rust 工作区编译检查：cargo check --workspace （退出码为 0 则 cargo_check_ok = true）\n"
            "3. Apple Swift 自检：cd apple && swift build -c release && ./.build/release/CodaSelfTest "
            "（退出码为 0 且输出通过信息则 apple_selftest_ok = true）\n"
            "在 details 中简要记录各项真实命令退出码与关键输出，并调用 finish 提交结果。"
        ),
        schema=GATE_SCHEMA,
        role="Build & Macro Gate",
        model="flash",
    )
    log(
        f"门禁结果: cargo_check={gate['cargo_check_ok']}, "
        f"macro_free={gate['macro_free_ok']}, "
        f"apple_selftest={gate['apple_selftest_ok']}"
    )
else:
    log("已按参数 run_self_test=false 跳过阶段 3 本地编译与 CodaSelfTest")

stacks_ok = all(r["aligned"] for r in reports)
gate_ok = True if gate is None else (
    gate["cargo_check_ok"] and gate["macro_free_ok"] and gate["apple_selftest_ok"]
)
overall_pass = stacks_ok and gate_ok

summary = {
    "overall_pass": overall_pass,
    "focus": focus or None,
    "contract": contract,
    "stack_reports": reports,
    "build_gate": gate,
}
log(f"动态工作流执行结束: overall_pass={overall_pass}")
print(json.dumps(summary, ensure_ascii=False, indent=2))
