"""Coda 全平台实时 Herdr 网格与 OrbStack 四大 Linux 发行版动态工作流。

可通过 Antigravity `run_workflow` 工具调用，支持以下 `Args`：
- distros (list[str], 可选): OrbStack Linux 发行版镜像列表，默认验证
  ["openeuler:24.03", "debian:12", "ubuntu:24.04", "centos:9-Stream"]。
- keep_vms (bool, 可选, 默认 False): 是否跳过 `finally` 阶段对临时进程/VM 的清理。
- agent_type (str, 可选, 默认 "self"): 调度子智能体类型。
- model (str, 可选, 默认 "flash"): 子智能体模型等级（如 "flash"、"inherit"、"pro"）。
"""

import json
import os
import re


DEFAULT_DISTROS = [
    ("openeuler:24.03", "coda-verify-openeuler"),
    ("debian:12", "coda-verify-debian"),
    ("ubuntu:24.04", "coda-verify-ubuntu"),
    ("centos:9-Stream", "coda-verify-centos"),
]

BUILD_SCHEMA = {
    "type": "object",
    "properties": {
        "target": {"type": "string"},
        "ok": {"type": "boolean"},
        "artifacts": {"type": "array", "items": {"type": "string"}},
        "detail": {"type": "string"},
    },
    "required": ["target", "ok", "artifacts", "detail"],
}

SETUP_SCHEMA = {
    "type": "object",
    "properties": {
        "ok": {"type": "boolean"},
        "server_local_url": {"type": "string"},
        "server_vm_url": {"type": "string"},
        "email": {"type": "string"},
        "password": {"type": "string"},
        "mac_device_name": {"type": "string"},
        "detail": {"type": "string"},
    },
    "required": [
        "ok",
        "server_local_url",
        "server_vm_url",
        "email",
        "password",
        "mac_device_name",
        "detail",
    ],
}

LINUX_PROVISION_SCHEMA = {
    "type": "object",
    "properties": {
        "distro_image": {"type": "string"},
        "vm_name": {"type": "string"},
        "os_pretty_name": {"type": "string"},
        "enrolled_ok": {"type": "boolean"},
        "service_ok": {"type": "boolean"},
        "detail": {"type": "string"},
    },
    "required": [
        "distro_image",
        "vm_name",
        "os_pretty_name",
        "enrolled_ok",
        "service_ok",
        "detail",
    ],
}

VERIFY_SCHEMA = {
    "type": "object",
    "properties": {
        "client_stack": {"type": "string"},
        "passed": {"type": "boolean"},
        "verified_host_count": {"type": "integer"},
        "verified_hosts": {"type": "array", "items": {"type": "string"}},
        "detail": {"type": "string"},
    },
    "required": [
        "client_stack",
        "passed",
        "verified_host_count",
        "verified_hosts",
        "detail",
    ],
}

CLEANUP_SCHEMA = {
    "type": "object",
    "properties": {
        "cleaned_ok": {"type": "boolean"},
        "remaining_verify_vms": {"type": "array", "items": {"type": "string"}},
        "user_daemon_config_untouched": {"type": "boolean"},
        "detail": {"type": "string"},
    },
    "required": [
        "cleaned_ok",
        "remaining_verify_vms",
        "user_daemon_config_untouched",
        "detail",
    ],
}


def resolve_distro_pairs(raw_distros):
    if not raw_distros:
        return list(DEFAULT_DISTROS)
    pairs = []
    default_map = dict(DEFAULT_DISTROS)
    for item in raw_distros:
        if isinstance(item, (list, tuple)) and len(item) >= 2:
            pairs.append((str(item[0]), str(item[1])))
        elif isinstance(item, dict) and item.get("image"):
            img = str(item["image"])
            name = str(item.get("vm_name") or default_map.get(img) or f"coda-verify-{re.sub(r'[^a-z0-9]+', '-', img.lower()).strip('-')}")
            pairs.append((img, name))
        else:
            img = str(item)
            name = default_map.get(img) or f"coda-verify-{re.sub(r'[^a-z0-9]+', '-', img.lower()).strip('-')}"
            pairs.append((img, name))
    return pairs


cfg = globals().get("args") or {}
keep_vms = bool(cfg.get("keep_vms", False))
agent_type = str(cfg.get("agent_type") or "self")
agent_model = str(cfg.get("model") or "flash")
distro_pairs = resolve_distro_pairs(cfg.get("distros"))
workspace_root = WORKSPACES[0] if WORKSPACES else os.getcwd()
cache_dir = os.path.expanduser("~/.cache/coda-verify-mesh")

phase("1. 并行构建 macOS 与 Linux aarch64 产物")
log(f"工作区={workspace_root}, 临时缓存={cache_dir}, Linux 发行版={distro_pairs}")

builds = []
setup_report = None
linux_reports = []
verify_reports = []
cleanup_report = None
workflow_error = None

try:
    build_targets = [
        (
            "macOS Host & Apple Swift 6",
            f"""你处于构建阶段，严禁修改任何 git 源码或 `~/.config/coda/daemon.json`，严禁执行 git commit/push。
请在工作区 `{workspace_root}` 运行：
```sh
set -e
export PATH="/opt/homebrew/bin:$HOME/.local/bin:/usr/local/bin:$PATH"
mkdir -p "{cache_dir}"
cargo build --release -p coda-server -p coda-daemon -p coda-desktop
cd "{workspace_root}/apple" && swift build -c release
test -x "{workspace_root}/target/release/coda-server"
test -x "{workspace_root}/target/release/coda-daemon"
test -x "{workspace_root}/apple/.build/release/CodaSelfTest"
```
全部构建且产物存在则 ok=true，列出构建好的关键二进制路径并调用 finish。""",
        ),
        (
            "Linux aarch64 in CentOS 9",
            f"""你处于跨发行版 Linux 构建阶段，严禁修改任何 git 源码或宿主机 `~/.config/coda/daemon.json`。
目标是在 OrbStack CentOS 9 Stream 虚拟机中构建兼容 openEuler/Debian/Ubuntu/CentOS 的 Linux `aarch64` `coda-daemon` 与 `herdr-core` (`hc`) 并缓存到 `{cache_dir}`：
1. 在 macOS 宿主机运行：
   ```sh
   set -e
   mkdir -p "{cache_dir}"
   orb delete -f coda-verify-centos >/dev/null 2>&1 || true
   orb create -a arm64 centos:9-Stream coda-verify-centos
   ```
2. 在 `coda-verify-centos` 内运行编译与安装命令（注意 CentOS 9 中 `curl` 可能与 `curl-minimal` 冲突，请使用 `--allowerasing`）：
   ```sh
   orb -m coda-verify-centos -u root sh -lc '
     set -eu
     dnf install -y --allowerasing gcc make git curl tar gzip xz unzip ca-certificates
     dnf install -y cargo rust >/dev/null 2>&1 || true
     set -- $(rustc --version 2>/dev/null || echo "rustc 0.0.0")
     rustc_ver="${{2:-0.0.0}}"
     major="${{rustc_ver%%.*}}"; rest="${{rustc_ver#*.}}"; minor="${{rest%%.*}}"
     if [ "${{major:-0}}" -lt 1 ] || [ "${{minor:-0}}" -lt 85 ]; then
       curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
       . "$HOME/.cargo/env"
     fi
     if [ -r "$HOME/.cargo/env" ]; then
       . "$HOME/.cargo/env"
     fi
     echo "rustc version: $(rustc --version)"
     curl -fsSL https://releases.herdr.dev/core/install.sh | sh
     export PATH="$HOME/.local/bin:$HOME/.herdr/bin:/usr/local/bin:$PATH"
     HC_BIN="$(command -v herdr-core || command -v hc || find /root /usr/local /home -maxdepth 4 -type f \\( -name herdr-core -o -name hc \\) 2>/dev/null | head -n1)"
     test -n "$HC_BIN"
     cd "{workspace_root}"
     CARGO_TARGET_DIR=/tmp/coda-linux-target cargo build --release -p coda-daemon
     cp /tmp/coda-linux-target/release/coda-daemon "{cache_dir}/coda-daemon-linux-aarch64"
     cp "$HC_BIN" "{cache_dir}/hc-linux-aarch64"
     chmod +x "{cache_dir}/coda-daemon-linux-aarch64" "{cache_dir}/hc-linux-aarch64"
     rm -rf /tmp/coda-linux-target
     "{cache_dir}/hc-linux-aarch64" --version || "{cache_dir}/hc-linux-aarch64" version || true
     "{cache_dir}/coda-daemon-linux-aarch64" --help >/dev/null
   '
   ```
3. 确认 `{cache_dir}/coda-daemon-linux-aarch64` 与 `{cache_dir}/hc-linux-aarch64` 已存在且可执行后，将结果通过 finish 返回。""",
        ),
    ]

    builds = await parallel(
        build_targets,
        lambda item: agent(
            prompt=item[1],
            schema=BUILD_SCHEMA,
            role=f"Build · {item[0]}",
            type_name=agent_type,
            model=agent_model,
        ),
    )
    for b in builds:
        log(f"[Build {b['target']}] ok={b['ok']} artifacts={len(b['artifacts'])}")

    if not all(b["ok"] for b in builds):
        raise RuntimeError(f"Phase 1 构建失败: {json.dumps(builds, ensure_ascii=False)}")
    phase("2. 启动隔离控制面与本机 Mac 混合宿主机")
    setup_report = await agent(
        prompt=f"""你负责启动隔离测试版 Coda 协调控制面与本机 macOS 混合宿主机（绝对禁止改动 `~/.config/coda/daemon.json` 或 `~/.local/bin`）：
1. 若存在旧的 `{cache_dir}/*.pid` 或 `{cache_dir}/mac-daemon/*.pid`，先安全终止对应旧进程。
2. 探测 OrbStack 虚拟机与 macOS 共同可达的地址并启动 `coda-server`（注意后台重定向 `</dev/null >...log 2>&1 &`，确保命令立即返回）：
   ```sh
   set -e
   mkdir -p "{cache_dir}/mac-bin" "{cache_dir}/mac-daemon" "{cache_dir}/mac-demo-repo"
   export PATH="/opt/homebrew/bin:$HOME/.local/bin:/usr/local/bin:$PATH"
   rm -f "{cache_dir}/coda-server.pid" "{cache_dir}/coda-server.log"

   HOST_ADDR="host.orb.internal"
   "{workspace_root}/target/release/coda-server" \\
     --env-file /dev/null \\
     --database-url postgres://vampire@localhost:5432/coda_test \\
     --http-bind 0.0.0.0:18080 \\
     --raw-tcp-bind 0.0.0.0:24445 \\
     --public-url "http://${{HOST_ADDR}}:18080" \\
     --raw-tcp-public-endpoint "${{HOST_ADDR}}:24445" \\
     --open-registration \\
     </dev/null >"{cache_dir}/coda-server.log" 2>&1 &
   echo $! >"{cache_dir}/coda-server.pid"

   for i in $(seq 1 30); do
     if curl -fsS http://127.0.0.1:18080/healthz >/dev/null 2>&1; then
       break
     fi
     sleep 0.2
   done
   curl -fsS http://127.0.0.1:18080/healthz >/dev/null

   if ! curl -fsS "http://${{HOST_ADDR}}:18080/healthz" >/dev/null 2>&1; then
     HOST_ADDR="$(ipconfig getifaddr en0)"
     kill "$(cat "{cache_dir}/coda-server.pid")" 2>/dev/null || true
     sleep 0.5
     "{workspace_root}/target/release/coda-server" \\
       --env-file /dev/null \\
       --database-url postgres://vampire@localhost:5432/coda_test \\
       --http-bind 0.0.0.0:18080 \\
       --raw-tcp-bind 0.0.0.0:24445 \\
       --public-url "http://${{HOST_ADDR}}:18080" \\
       --raw-tcp-public-endpoint "${{HOST_ADDR}}:24445" \\
       --open-registration \\
       </dev/null >"{cache_dir}/coda-server.log" 2>&1 &
     echo $! >"{cache_dir}/coda-server.pid"
     sleep 1
     curl -fsS "http://${{HOST_ADDR}}:18080/healthz" >/dev/null
   fi
   echo "HOST_ADDR=$HOST_ADDR"
   ```
3. 通过开放注册接口创建本轮唯一验证账号（避免数据库残留用户冲突）：
   ```sh
   TS="$(date +%s)"
   EMAIL="mesh-${{TS}}@coda.dev"
   PASSWORD="MeshPass2026!"
   curl -fsS -X POST http://127.0.0.1:18080/api/v1/auth/register \\
     -H 'Content-Type: application/json' \\
     -d "{{\\"email\\":\\"$EMAIL\\",\\"handle\\":\\"mesh$TS\\",\\"display_name\\":\\"Mesh Verifier\\",\\"password\\":\\"$PASSWORD\\"}}" >/dev/null
   ```
4. 初始化本机测试 Git 仓库（带脏改动以验证 GitDiff）并以隔离配置注册启动 Mac `hybrid` 宿主机：
   ```sh
   cd "{cache_dir}/mac-demo-repo"
   if [ ! -d .git ]; then
     git init
     git config user.email mac@coda.dev
     git config user.name MacHost
     echo "# Coda Mac Demo Repo" > README.md
     git add README.md
     git commit -m init
   fi
   echo "dirty_mac_mesh_line=1" >> README.md

   CODA_INSTALL_DIR="{cache_dir}/mac-bin" \\
     sh "{workspace_root}/deploy/install-daemon.sh" \\
     --config "{cache_dir}/mac-daemon/daemon.json" \\
     --server http://127.0.0.1:18080 \\
     --local-bin "{workspace_root}/target/release/coda-daemon" \\
     --email "$EMAIL" \\
     --password "$PASSWORD" \\
     --name macos-local-host \\
     --role hybrid \\
     --p2p-bind 0.0.0.0:24454 \\
     --non-interactive </dev/null
   ```
5. 验证 `{cache_dir}/mac-daemon/daemon.json` 与 `{cache_dir}/mac-daemon/coda-daemon.pid` 存活后，返回结构化 JSON：`server_local_url="http://127.0.0.1:18080"`, `server_vm_url=f"http://${{HOST_ADDR}}:18080"`, `email`, `password`, `mac_device_name="macos-local-host"`, `ok=true`。""",
        schema=SETUP_SCHEMA,
        role="Setup · Coordinator & macOS Hybrid Host",
        type_name=agent_type,
        model=agent_model,
    )
    log(
        f"控制面就绪: ok={setup_report['ok']}, vm_url={setup_report['server_vm_url']}, "
        f"account={setup_report['email']}, mac={setup_report['mac_device_name']}"
    )
    if not setup_report["ok"]:
        raise RuntimeError(f"Phase 2 控制面或本机 Mac 启动失败: {setup_report}")

    phase("3. OrbStack 四大 Linux 发行版并行装机与实时入网")
    server_vm_url = setup_report["server_vm_url"]
    email = setup_report["email"]
    password = setup_report["password"]

    def build_linux_task(pair):
        distro_image, vm_name = pair
        return agent(
            prompt=f"""请将 Linux 发行版 `{distro_image}`（OrbStack 实例 `{vm_name}`）作为真实远程开发服务器安装 Coda Daemon 与 herdr-core 并实时接入控制面：
- 目标协调控制面（供 VM 访问）：`{server_vm_url}`
- 登录邮箱：`{email}`
- 登录密码：`{password}`
- Linux 二进制缓存：`{cache_dir}/coda-daemon-linux-aarch64` 与 `{cache_dir}/hc-linux-aarch64`

执行步骤：
1. 检查 `orb list` 中是否存在 `{vm_name}`；若不存在，执行创建：
   `orb list | awk '{{print $1}}' | grep -qx "{vm_name}" || orb create -a arm64 "{distro_image}" "{vm_name}"`
2. 通过 `orb -m {vm_name} -u root sh -lc '...'` 在虚拟机内完成：
   ```sh
   set -eu
   mkdir -p /root/demo-repo
   # 若系统暂无 git，install-daemon.sh 会自动安装；为保证 demo-repo 初始化，先确保安装基础 git/curl/tar
   if ! command -v git >/dev/null 2>&1; then
     if command -v dnf >/dev/null 2>&1; then
       dnf install -y --allowerasing git curl tar ca-certificates
     elif command -v apt-get >/dev/null 2>&1; then
       DEBIAN_FRONTEND=noninteractive apt-get update -y >/dev/null
       DEBIAN_FRONTEND=noninteractive apt-get install -y git curl tar ca-certificates
     fi
   fi
   cd /root/demo-repo
   if [ ! -d .git ]; then
     git init
     git config user.email root@coda.dev
     git config user.name Root
     echo "# {vm_name}" > README.md
     git add README.md
     git commit -m init
   fi
   echo "dirty_{vm_name}=1" >> README.md

   sh "{workspace_root}/deploy/install-daemon.sh" \\
     --config /root/.config/coda/daemon.json \\
     --server "{server_vm_url}" \\
     --local-bin "{cache_dir}/coda-daemon-linux-aarch64" \\
     --local-hc-bin "{cache_dir}/hc-linux-aarch64" \\
     --email "{email}" \\
     --password "{password}" \\
     --name "{vm_name}" \\
     --role daemon \\
     --non-interactive </dev/null

   export PATH="/root/.local/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
   hc --version || hc version || true
   coda-daemon --config /root/.config/coda/daemon.json status
   . /etc/os-release || true
   echo "OS_PRETTY_NAME=${{PRETTY_NAME:-{distro_image}}}"
   ```
3. 确认上述命令全部退出码为 0、`coda-daemon` 后台运行且提取真实 `OS_PRETTY_NAME`，通过 finish 返回结构化 JSON。""",
            schema=LINUX_PROVISION_SCHEMA,
            role=f"Provision · {vm_name}",
            type_name=agent_type,
            model=agent_model,
        )

    linux_reports = await parallel(distro_pairs, build_linux_task)
    for lr in linux_reports:
        log(
            f"[Linux {lr['vm_name']}] os={lr['os_pretty_name']} | "
            f"enrolled={lr['enrolled_ok']} | service={lr['service_ok']}"
        )

    if not all(r["enrolled_ok"] and r["service_ok"] for r in linux_reports):
        raise RuntimeError(
            f"Phase 3 Linux 装机未全部成功: {json.dumps(linux_reports, ensure_ascii=False)}"
        )

    phase("4. Rust 桌面端与 Apple Swift 6 移动端并行全链路实测")
    server_local_url = setup_report["server_local_url"]
    expected_hosts = 1 + len(distro_pairs)
    log(f"开始并行验证 {expected_hosts} 台在线主机（1 Mac + {len(distro_pairs)} Linux）")

    verify_specs = [
        (
            "Rust Desktop Mesh",
            f"""你处于真机网格验证阶段，严禁修改任何源码。
请运行 Rust 桌面端实时网格自检工具验证全部 {expected_hosts} 台在线主机（包含本机 Mac 与 4 大 Linux 发行版）：
```sh
set -e
export PATH="/opt/homebrew/bin:$HOME/.local/bin:/usr/local/bin:$PATH"
"{workspace_root}/target/release/coda-daemon" verify-mesh \\
  --server "{server_local_url}" \\
  --email "{email}" \\
  --password "{password}" \\
  --expect-hosts {expected_hosts}
```
核对输出的 JSON 中 `"ok": true` 且 `verified_hosts` 数量 >= {expected_hosts}（同时检查所有主机的 `checked_diff`、`route` 与 `terminal`），通过 finish 返回结构化 JSON。""",
        ),
        (
            "Apple Swift 6 Mobile & Mac Client",
            f"""你处于 Apple Swift 6 移动端与桌面端实时网格验证阶段，严禁修改任何源码，也不要重新运行 `swift build`（阶段 1 已构建好 release 二进制）。
请直接运行实时网格模式下的 Apple 原生测试套件：
```sh
set -e
CODA_LIVE_MESH_TEST_URL="{server_local_url}" \\
CODA_LIVE_MESH_EMAIL="{email}" \\
CODA_LIVE_MESH_PASSWORD="{password}" \\
CODA_EXPECTED_MIN_HOSTS={expected_hosts} \\
  "{workspace_root}/apple/.build/release/CodaSelfTest"
```
确认输出包含 `[OK] CODA APPLE LIVE MESH VERIFIED ACROSS {expected_hosts} HOSTS!` 与 `[OK] ALL CODA APPLE NATIVE SELF-TESTS PASSED!`，将已验证的主机数与详情通过 finish 提交。""",
        ),
    ]

    verify_reports = await parallel(
        verify_specs,
        lambda item: agent(
            prompt=item[1],
            schema=VERIFY_SCHEMA,
            role=f"Verify · {item[0]}",
            type_name=agent_type,
            model=agent_model,
        ),
    )
    for vr in verify_reports:
        log(
            f"[Verify {vr['client_stack']}] passed={vr['passed']} | "
            f"hosts={vr['verified_host_count']}"
        )

except Exception as exc:
    workflow_error = str(exc)
    log(f"工作流执行遇到异常（即将进入确定性清理阶段）: {workflow_error}")

finally:
    phase("5. 确定性环境回收与零残留校验")
    if keep_vms:
        log("按参数 keep_vms=true 跳过临时虚拟机与缓存目录清理。")
        cleanup_report = {
            "cleaned_ok": True,
            "remaining_verify_vms": [],
            "user_daemon_config_untouched": True,
            "detail": "Skipped teardown because keep_vms=true",
        }
    else:
        vm_names_str = " ".join(name for _, name in distro_pairs)
        cleanup_report = await agent(
            prompt=f"""请清理本轮动态工作流创建的所有临时进程、OrbStack 虚拟机与临时缓存目录，并核验宿主机环境零残留：
1. 停止隔离版 `coda-server` 与本机 `macos-local-host` `coda-daemon`（读取 `{cache_dir}/coda-server.pid` 与 `{cache_dir}/mac-daemon/coda-daemon.pid` 并 `kill`，若仍占用 18080/24445/24454 端口一并清理测试进程）。
2. 删除全部临时 OrbStack 虚拟机：
   `orb delete -f {vm_names_str} >/dev/null 2>&1 || true`
3. 删除临时缓存目录：
   `rm -rf "{cache_dir}"`
4. 检查：
   - 运行 `orb list`，确认没有任何 `coda-verify-*` 残留虚拟机；
   - 确认用户个人目录 `~/.config/coda/daemon.json` 未被本测试写入或破坏（可用 `ls -la "$HOME/.config/coda/daemon.json" 2>/dev/null || echo absent` 查看）。
将校验结果通过 finish 返回结构化 JSON。""",
            schema=CLEANUP_SCHEMA,
            role="Teardown · Processes, VMs & Cache",
            type_name=agent_type,
            model=agent_model,
        )
        log(
            f"环境回收完成: cleaned_ok={cleanup_report['cleaned_ok']}, "
            f"remaining_vms={cleanup_report['remaining_verify_vms']}, "
            f"user_cfg_untouched={cleanup_report['user_daemon_config_untouched']}"
        )

overall_pass = (
    workflow_error is None
    and setup_report is not None
    and setup_report["ok"]
    and len(linux_reports) == len(distro_pairs)
    and all(r["enrolled_ok"] and r["service_ok"] for r in linux_reports)
    and len(verify_reports) == 2
    and all(v["passed"] for v in verify_reports)
    and cleanup_report is not None
    and cleanup_report["cleaned_ok"]
)

summary = {
    "overall_pass": overall_pass,
    "error": workflow_error,
    "builds": builds,
    "coordinator": setup_report,
    "linux_hosts": linux_reports,
    "verification": verify_reports,
    "cleanup": cleanup_report,
}
log(f"动态工作流总结: overall_pass={overall_pass}")
print(json.dumps(summary, ensure_ascii=False, indent=2))
