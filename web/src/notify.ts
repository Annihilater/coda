import type { HerdrTerminalStatus, Locale } from "./types";

export interface TerminalNotifyInput {
  daemonId: string;
  daemonName: string;
  terminalAddr: string;
  terminalName?: string | null;
  status?: HerdrTerminalStatus | null;
}

export interface TerminalNotificationPayload {
  key: string;
  daemonId: string;
  terminalAddr: string;
  kind: "needs_you" | "error" | "done";
  title: string;
  body: string;
}

export interface NotifyObserveContext {
  activeDaemonId: string | null;
  activeTerminalAddr: string | null;
  documentHidden: boolean;
  locale: Locale;
  nowMs?: number;
}

function normalizeState(raw?: string | null): string {
  return String(raw || "idle").trim().toLowerCase();
}

export function isNeedsYouState(raw?: string | null): boolean {
  const s = normalizeState(raw);
  return (
    s === "needs_you" ||
    s === "needsyou" ||
    s === "needs-you" ||
    s === "blocked" ||
    s === "waiting" ||
    s === "permission" ||
    s === "attention"
  );
}

/**
 * Edge-triggered OSC 7501 notification tracker:
 * - Fires ONLY on state transitions into `needs_you` (including aliases), `error`, or `working -> done`/`ready`.
 * - Suppresses alerts when the page is visible AND the user is already viewing that exact terminal.
 * - Enforces a per-terminal cooldown so bursty agent output never spams OS notifications.
 */
export class TerminalNotificationTracker {
  private lastState = new Map<string, string>();
  private lastNotifiedAt = new Map<string, number>();
  private readonly cooldownMs: number;

  constructor(cooldownMs = 20_000) {
    this.cooldownMs = cooldownMs;
  }

  reset(): void {
    this.lastState.clear();
    this.lastNotifiedAt.clear();
  }

  observe(
    input: TerminalNotifyInput,
    ctx: NotifyObserveContext
  ): TerminalNotificationPayload | null {
    const key = `${input.daemonId}:${input.terminalAddr}`;
    const rawState = normalizeState(input.status?.state);
    const nextState = isNeedsYouState(rawState) ? "needs_you" : rawState;
    const prevState = this.lastState.get(key);
    this.lastState.set(key, nextState);

    // First snapshot establishes baseline without spamming on initial page load.
    if (prevState === undefined) {
      return null;
    }
    if (prevState === nextState) {
      return null;
    }

    let kind: "needs_you" | "error" | "done" | null = null;
    if (nextState === "needs_you") {
      kind = "needs_you";
    } else if (nextState === "error" || nextState === "failed") {
      kind = "error";
    } else if (prevState === "working" && (nextState === "done" || nextState === "ready" || nextState === "idle")) {
      kind = "done";
    }

    if (!kind) return null;

    const viewingSameTerminal =
      !ctx.documentHidden &&
      ctx.activeDaemonId === input.daemonId &&
      ctx.activeTerminalAddr === input.terminalAddr;
    if (viewingSameTerminal) {
      return null;
    }

    const now = ctx.nowMs ?? Date.now();
    const lastTs = this.lastNotifiedAt.get(key);
    if (lastTs !== undefined && now - lastTs < this.cooldownMs) {
      return null;
    }
    this.lastNotifiedAt.set(key, now);

    const appLabel =
      input.status?.app || input.terminalName || input.terminalAddr.split("/").pop() || "Terminal";
    const machineLabel = input.daemonName || "Host";
    const detail = (input.status?.msg || input.status?.title || "").trim();

    if (kind === "needs_you") {
      return {
        key,
        daemonId: input.daemonId,
        terminalAddr: input.terminalAddr,
        kind,
        title:
          ctx.locale === "zh"
            ? `需要审批 · ${appLabel} (${machineLabel})`
            : `Action Needed · ${appLabel} (${machineLabel})`,
        body:
          detail ||
          (ctx.locale === "zh"
            ? "AI Agent 正在等待你的交互确认（点击立即跳转终端）"
            : "AI Agent is waiting for your approval (click to open terminal)"),
      };
    }

    if (kind === "error") {
      return {
        key,
        daemonId: input.daemonId,
        terminalAddr: input.terminalAddr,
        kind,
        title:
          ctx.locale === "zh"
            ? `任务异常 · ${appLabel} (${machineLabel})`
            : `Task Error · ${appLabel} (${machineLabel})`,
        body:
          detail ||
          (ctx.locale === "zh" ? "终端任务返回异常状态，点击查看详情" : "Terminal reported an error — click to inspect"),
      };
    }

    return {
      key,
      daemonId: input.daemonId,
      terminalAddr: input.terminalAddr,
      kind,
      title:
        ctx.locale === "zh"
          ? `任务完成 · ${appLabel} (${machineLabel})`
          : `Task Completed · ${appLabel} (${machineLabel})`,
      body:
        detail ||
        (ctx.locale === "zh" ? "后台 Agent 已完成当前工作" : "Background agent finished its run"),
    };
  }
}

export type BrowserNotificationPermissionState = "unsupported" | "default" | "granted" | "denied";

export function getBrowserNotificationPermission(): BrowserNotificationPermissionState {
  if (typeof window === "undefined" || typeof Notification === "undefined") {
    return "unsupported";
  }
  return Notification.permission as BrowserNotificationPermissionState;
}

export async function requestBrowserNotificationPermission(): Promise<BrowserNotificationPermissionState> {
  if (typeof window === "undefined" || typeof Notification === "undefined") {
    return "unsupported";
  }
  if (Notification.permission === "granted" || Notification.permission === "denied") {
    return Notification.permission as BrowserNotificationPermissionState;
  }
  try {
    const res = await Notification.requestPermission();
    return res as BrowserNotificationPermissionState;
  } catch {
    return getBrowserNotificationPermission();
  }
}

export function dispatchBrowserNotification(
  payload: TerminalNotificationPayload,
  onSelect: () => void
): boolean {
  if (typeof window === "undefined" || typeof Notification === "undefined") return false;
  if (Notification.permission !== "granted") return false;
  try {
    const n = new Notification(payload.title, {
      body: payload.body,
      tag: `coda-${payload.key}`,
      silent: payload.kind === "done",
    });
    n.onclick = () => {
      try {
        window.focus();
      } catch {
        // ignore focus errors
      }
      onSelect();
      n.close();
    };
    return true;
  } catch {
    return false;
  }
}
