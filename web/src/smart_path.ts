import type { Device, TransportMode } from "./types";

export interface LocationLike {
  protocol: string;
  hostname: string;
}

export interface SmartPathProbePlan {
  delayMs: number;
  consecutiveFailures: number;
}

/**
 * Collects and prioritizes direct WebSocket candidates for a daemon while enforcing
 * browser mixed-content security rules (HTTPS pages only allow `wss://`, or loopback `ws://`
 * when the page itself is served from localhost/127.0.0.1).
 */
export function collectDirectWsCandidates(peer: Device, loc: LocationLike): string[] {
  const out: string[] = [];
  const isLocalOrigin = loc.hostname === "127.0.0.1" || loc.hostname === "localhost";
  const pushUnique = (u: string) => {
    const trimmed = u.trim();
    if (!trimmed) return;
    if (loc.protocol === "https:") {
      const isLocalWs =
        isLocalOrigin && (trimmed.startsWith("ws://127.0.0.1") || trimmed.startsWith("ws://localhost"));
      if (!trimmed.startsWith("wss://") && !isLocalWs) return;
    }
    if (!out.includes(trimmed)) out.push(trimmed);
  };

  // Prefer explicit WebSocket endpoints first (`lan_ws` -> `ipv6_ws`), then raw IPv4/IPv6 LAN addresses.
  const endpoints = peer.endpoints_json || [];
  for (const ep of endpoints) {
    if (ep.kind === "lan_ws") pushUnique(ep.addr);
  }
  for (const ep of endpoints) {
    if (ep.kind === "ipv6_ws") pushUnique(ep.addr);
  }
  for (const ep of endpoints) {
    if (ep.kind === "lan_ipv4") pushUnique(`ws://${ep.addr}`);
  }
  for (const ep of endpoints) {
    if (ep.kind === "cn_public_ipv6") pushUnique(`ws://${ep.addr}`);
  }

  if (isLocalOrigin) {
    pushUnique("ws://127.0.0.1:24444");
  }
  return out;
}

/**
 * Pure state machine governing background LAN Direct <-> Smart Relay hot-switch probing:
 * - Exponential backoff (`15s -> 30s -> 60s -> 120s`) while Relay is active and LAN is unreachable.
 * - Immediate probe reset on OS network transition (`online` / `navigator.connection` change / tab resume).
 * - Flap cooldown (`8s`) after a direct drop before re-attempting upgrade so flaky Wi-Fi never oscillates.
 */
export class SmartPathBackoffPolicy {
  private failuresByPeer = new Map<string, number>();
  private lastDropAtByPeer = new Map<string, number>();
  private readonly baseIntervalMs: number;
  private readonly maxIntervalMs: number;
  private readonly dropCooldownMs: number;

  constructor(opts: { baseIntervalMs?: number; maxIntervalMs?: number; dropCooldownMs?: number } = {}) {
    this.baseIntervalMs = opts.baseIntervalMs ?? 15_000;
    this.maxIntervalMs = opts.maxIntervalMs ?? 120_000;
    this.dropCooldownMs = opts.dropCooldownMs ?? 8_000;
  }

  recordUpgradeSuccess(peerId: string): void {
    this.failuresByPeer.set(peerId, 0);
  }

  recordProbeFailure(peerId: string): SmartPathProbePlan {
    const fails = (this.failuresByPeer.get(peerId) ?? 0) + 1;
    this.failuresByPeer.set(peerId, fails);
    const delayMs = Math.min(this.maxIntervalMs, this.baseIntervalMs * Math.pow(2, Math.min(fails - 1, 4)));
    return { delayMs, consecutiveFailures: fails };
  }

  recordDirectDrop(peerId: string, nowMs = Date.now()): SmartPathProbePlan {
    this.lastDropAtByPeer.set(peerId, nowMs);
    this.failuresByPeer.set(peerId, 1);
    return { delayMs: Math.max(this.dropCooldownMs, this.baseIntervalMs), consecutiveFailures: 1 };
  }

  resetForNetworkChange(peerId?: string): void {
    if (peerId) {
      this.failuresByPeer.delete(peerId);
    } else {
      this.failuresByPeer.clear();
    }
  }

  canProbeNow(peerId: string, currentMode: TransportMode, nowMs = Date.now()): boolean {
    if (currentMode === "lan_ws") return false;
    const lastDrop = this.lastDropAtByPeer.get(peerId);
    return lastDrop === undefined || nowMs - lastDrop >= this.dropCooldownMs;
  }

  nextDelayMs(peerId: string): number {
    const fails = this.failuresByPeer.get(peerId) ?? 0;
    if (fails <= 0) return this.baseIntervalMs;
    return Math.min(this.maxIntervalMs, this.baseIntervalMs * Math.pow(2, Math.min(fails - 1, 4)));
  }
}
