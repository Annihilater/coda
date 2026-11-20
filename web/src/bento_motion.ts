import { clamp01, soft, springFromRest } from "./icons";

/**
 * Drives the 16-second periodic Bento motion grid on the landing page with
 * automatic `IntersectionObserver` pausing when scrolled offscreen and `prefers-reduced-motion` support.
 */
export class BentoMotionController {
  private rafId: number | null = null;
  private observer: IntersectionObserver | null = null;
  private visible = true;
  private startMs = typeof performance !== "undefined" ? performance.now() : 0;

  stop(): void {
    if (this.rafId !== null && typeof cancelAnimationFrame !== "undefined") {
      cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    if (this.observer) {
      this.observer.disconnect();
      this.observer = null;
    }
  }

  start(stageId: string, isActiveRoute: () => boolean): void {
    this.stop();
    if (typeof document === "undefined" || typeof window === "undefined") return;
    const stage = document.getElementById(stageId);
    if (!stage || !isActiveRoute()) return;

    const glider = stage.querySelector<HTMLElement>('[data-bento="route-glider"]');
    const rttEl = stage.querySelector<HTMLElement>('[data-bento="rtt-val"]');
    const badgeLan = stage.querySelector<HTMLElement>('[data-bento="badge-lan"]');
    const badgeWss = stage.querySelector<HTMLElement>('[data-bento="badge-wss"]');
    const sparkLan = stage.querySelector<SVGPathElement>('[data-bento="spark-lan"]');
    const sparkWss = stage.querySelector<SVGPathElement>('[data-bento="spark-wss"]');
    const sealRing = stage.querySelector<SVGCircleElement>('[data-bento="seal-ring"]');
    const sealPlain = stage.querySelector<HTMLElement>('[data-bento="seal-plain"]');
    const sealCipher = stage.querySelector<HTMLElement>('[data-bento="seal-cipher"]');
    const sealPct = stage.querySelector<HTMLElement>('[data-bento="seal-pct"]');
    const sealSteps = Array.from(stage.querySelectorAll<HTMLElement>("[data-seal-step]"));
    const otpCells = Array.from(stage.querySelectorAll<HTMLElement>("[data-otp-idx]"));
    const otpPending = stage.querySelector<HTMLElement>('[data-bento="otp-pending"]');
    const otpApproved = stage.querySelector<HTMLElement>('[data-bento="otp-approved"]');
    const ptyLines = Array.from(stage.querySelectorAll<HTMLElement>("[data-pty-idx]"));
    const pktA = stage.querySelector<SVGCircleElement>('[data-bento="pkt-a"]');
    const pktB = stage.querySelector<SVGCircleElement>('[data-bento="pkt-b"]');
    const voiceBars = Array.from(stage.querySelectorAll<HTMLElement>("[data-voice-idx]"));
    const reducedMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const CIRC = 113.1;

    const paintFrame = (nowMs: number) => {
      const l = reducedMotion ? 1.5 : ((nowMs - this.startMs) / 1000) % 16;

      // 1. SmartPath Route Glider + Sparkline Crossfade
      let routeT = 0;
      if (l >= 1.5 && l < 2.9) {
        routeT = springFromRest((l - 1.5) / 1.4);
      } else if (l >= 2.9 && l < 9.0) {
        routeT = 1;
      } else if (l >= 9.0 && l < 10.4) {
        routeT = 1 - springFromRest((l - 9.0) / 1.4);
      }
      const crossFade = clamp01(routeT);
      if (glider) glider.style.transform = `translate3d(${routeT * 100}%, 0, 0)`;
      if (rttEl) rttEl.textContent = `${(0.8 + crossFade * 25.6).toFixed(1)} ms`;
      if (badgeLan) badgeLan.style.opacity = `${1 - soft(crossFade)}`;
      if (badgeWss) badgeWss.style.opacity = `${soft(crossFade)}`;
      if (sparkLan) sparkLan.style.opacity = `${(1 - soft(crossFade) * 0.72).toFixed(3)}`;
      if (sparkWss) sparkWss.style.opacity = `${(0.28 + soft(crossFade) * 0.72).toFixed(3)}`;

      // 2. Zero-Knowledge E2EE Seal Ring + 3-Stage Pipeline Highlight
      let sealP = 0;
      if (l >= 3.0 && l < 4.5) {
        sealP = springFromRest((l - 3.0) / 1.5);
      } else if (l >= 4.5 && l < 12.2) {
        sealP = 1;
      } else if (l >= 12.2 && l < 13.7) {
        sealP = 1 - soft((l - 12.2) / 1.5);
      }
      const clampedSeal = clamp01(sealP);
      if (sealRing) sealRing.style.strokeDashoffset = `${CIRC * (1 - clampedSeal)}`;
      if (sealPlain) sealPlain.style.opacity = `${1 - soft(clampedSeal)}`;
      if (sealCipher) sealCipher.style.opacity = `${soft(clampedSeal)}`;
      if (sealPct) sealPct.textContent = `${Math.round(clampedSeal * 100)}%`;
      sealSteps.forEach((step, idx) => {
        const active = clampedSeal >= idx * 0.34 + 0.05;
        step.style.borderColor = active
          ? idx === 2
            ? "var(--emerald-bd)"
            : "var(--signal-blue-bd)"
          : "var(--border-primary)";
        step.style.color = active
          ? idx === 2
            ? "var(--emerald)"
            : "var(--text-primary)"
          : "var(--text-tertiary)";
      });

      // 3. RFC 8628 Device Code sequential reveal + approval state
      let approvedP = 0;
      if (l >= 7.2 && l < 8.4) {
        approvedP = soft((l - 7.2) / 1.2);
      } else if (l >= 8.4 && l < 13.5) {
        approvedP = 1;
      } else if (l >= 13.5 && l < 14.8) {
        approvedP = 1 - soft((l - 13.5) / 1.3);
      }
      if (otpPending) otpPending.style.opacity = `${1 - approvedP}`;
      if (otpApproved) otpApproved.style.opacity = `${approvedP}`;
      otpCells.forEach((cell, idx) => {
        const focusStart = 1.0 + idx * 0.7;
        const focusEnd = focusStart + 0.7;
        const dist =
          l >= focusStart && l <= focusEnd
            ? 1 - Math.abs(l - (focusStart + 0.35)) / 0.35
            : 0;
        const isLit = dist > 0.1 || approvedP > 0.5;
        cell.style.borderColor = isLit ? "var(--signal-blue)" : "var(--border-primary)";
        cell.style.transform = `translate3d(0, ${-2 * Math.max(0, dist)}px, 0)`;
      });

      // 4. Live PTY Stream subtle active line pulse
      ptyLines.forEach((line, idx) => {
        const center = 2.5 + idx * 4.5;
        const delta = Math.abs(l - center);
        const w = delta < 2.2 ? soft(1 - delta / 2.2) : 0;
        line.style.backgroundColor = `rgba(255, 255, 255, ${(0.03 + 0.06 * w).toFixed(3)})`;
        line.style.opacity = `${(0.72 + 0.28 * w).toFixed(3)}`;
      });

      // 5. Encrypted Mesh Topology Packet Dots
      const phaseA = 0.5 - 0.5 * Math.cos((2 * Math.PI * l) / 8);
      const phaseB = 0.5 - 0.5 * Math.cos((2 * Math.PI * ((l + 4) % 16)) / 8);
      if (pktA) {
        const ax = 54 + phaseA * 102;
        const ay = 29 - Math.sin(phaseA * Math.PI) * 14;
        pktA.setAttribute("cx", ax.toFixed(1));
        pktA.setAttribute("cy", ay.toFixed(1));
      }
      if (pktB) {
        const bx = 190 + phaseB * 102;
        const by = 29 + Math.sin(phaseB * Math.PI) * 14;
        pktB.setAttribute("cx", bx.toFixed(1));
        pktB.setAttribute("cy", by.toFixed(1));
      }

      // 6. Voice-to-Terminal periodic 16s-locked bars
      const speechEnv = 0.28 + 0.72 * (0.5 - 0.5 * Math.cos((2 * Math.PI * l) / 16));
      voiceBars.forEach((bar, idx) => {
        const harmonic = 0.5 + 0.5 * Math.sin(((3 + (idx % 3)) * 2 * Math.PI * l) / 16 + idx * 0.9);
        const sy = 0.2 + 0.8 * speechEnv * harmonic;
        bar.style.transform = `scaleY(${sy.toFixed(3)})`;
        bar.style.opacity = `${(0.45 + 0.55 * sy).toFixed(3)}`;
      });
    };

    paintFrame(performance.now());
    if (reducedMotion) return;

    const tick = (now: number) => {
      if (!this.visible || !isActiveRoute()) {
        this.rafId = null;
        return;
      }
      paintFrame(now);
      this.rafId = requestAnimationFrame(tick);
    };

    if (typeof IntersectionObserver !== "undefined") {
      this.observer = new IntersectionObserver(
        (entries) => {
          const visible = entries.some((entry) => entry.isIntersecting);
          this.visible = visible;
          if (visible && this.rafId === null && isActiveRoute()) {
            this.rafId = requestAnimationFrame(tick);
          }
        },
        { threshold: 0.08 }
      );
      this.observer.observe(stage);
    }

    this.rafId = requestAnimationFrame(tick);
  }
}
