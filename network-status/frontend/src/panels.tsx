import { createEffect } from "solid-js";
import { sparkline } from "./chart";
import { fmtDur, fmtMem, fmtRate } from "./format";
import type { Snapshot } from "./api";
import type { Store } from "./store";

export function Header(s: { store: Store }) {
  let titleEl: HTMLElement | undefined;
  let metaEl: HTMLElement | undefined;
  let pillEl: HTMLElement | undefined;

  createEffect(() => {
    const snap = s.store.last();
    const cfg = s.store.config();
    if (titleEl) titleEl.textContent = cfg.title;

    if (metaEl && snap) {
      const r = snap.router;
      metaEl.textContent =
        `router up ${fmtDur(r.uptime_secs)}` +
        (r.nprocs != null ? ` · ${r.nprocs} CPUs` : "") +
        (r.cpu.user != null ? ` · cpu ${(100 - (r.cpu.idle ?? 0)).toFixed(1)}%` : "") +
        (r.mem.free != null ? ` · ${fmtMem(r.mem.free)} mem free` : "") +
        ` · ${new Date(snap.ts * 1000).toTimeString().slice(0, 8)}`;
    }

    if (pillEl && snap) {
      if (!snap.ok) {
        pillEl.textContent = "router unreachable";
        pillEl.className = "pill bad";
      } else if (snap.issues.some((i) => i.level === "bad")) {
        pillEl.textContent = "problem detected";
        pillEl.className = "pill bad";
      } else if (snap.issues.length) {
        pillEl.textContent = `${snap.issues.length} warning${snap.issues.length > 1 ? "s" : ""}`;
        pillEl.className = "pill warn";
      } else {
        pillEl.textContent = "all good";
        pillEl.className = "pill good";
      }
    }
  });

  return (
    <header>
      <h1> <span ref={el => { titleEl = el; }}></span></h1>
      <span class="sub mono" ref={el => { metaEl = el; }}></span>
      <span class="pill good" ref={el => { pillEl = el; }} style="margin-left:auto">waiting…</span>
    </header>
  );
}

export function Banner(s: { store: Store }) {
  let el: HTMLElement | undefined;

  createEffect(() => {
    const snap = s.store.last();
    if (!el || !snap || snap.ok) {
      if (el) el.style.display = "none";
      return;
    }
    el.style.display = "block";
    el.textContent = `Cannot reach the router (10.3.1.1) — ${snap.error || "unknown error"}. Showing last known data; will retry automatically.`;
  });

  return <div class="banner" ref={node => { el = node; }}></div>;
}

export function IfaceCards(s: { store: Store }) {
  let grid: HTMLElement | undefined;

  createEffect(() => {
    const snap = s.store.last();
    const gridEl = grid;
    if (!snap || !gridEl) return;
    const wan = s.store.config().wan;

    const cards = interestingIfaces(snap)
      .map((i) => ({
        i,
        up: s.store.hist().rates[i.name]?.up ?? [],
        down: s.store.hist().rates[i.name]?.down ?? [],
      }))
      .sort((a, b) => (b.i.down_total + b.i.up_total) - (a.i.down_total + a.i.up_total));

    gridEl.replaceChildren(
      ...cards.map((c) => {
        const wrap = document.createElement("div");
        wrap.className = "iface";
        wrap.innerHTML =
          `<div class="head"><span class="dot ${c.i.active ? "up" : "down"}"></span>` +
          `<span class="name">${c.i.name}${c.i.name === wan ? ' <span class="muted">(WAN)</span>' : ""}</span>` +
          `<span class="muted mono" style="margin-left:auto">${c.i.active ? "linked" : "no carrier"}</span></div>` +
          `<canvas></canvas>` +
          `<div class="rates mono"><span class="upc">▲ ${fmtRate(c.i.up_bps)}</span>` +
          `<span class="downc">▼ ${fmtRate(c.i.down_bps)}</span></div>`;
        return wrap;
      }),
    );

    const canvases = Array.from(gridEl.querySelectorAll<HTMLCanvasElement>("canvas"));
    cards.forEach((c, idx) => {
      const node = canvases[idx];
      if (node) {
        sparkline(node, [
          { data: c.down, color: "#60a5fa" },
          { data: c.up, color: "#2dd4bf" },
        ]);
      }
    });
  });

  return <div class="iface-grid" ref={node => { grid = node; }}></div>;
}

export function Issues(s: { store: Store }) {
  let el: HTMLElement | undefined;

  createEffect(() => {
    const snap = s.store.last();
    if (!el) return;
    if (!snap || !snap.issues.length) {
      el.innerHTML = '<span class="muted">✔ no issues detected</span>';
      return;
    }
    el.innerHTML = snap.issues
      .map((i) => `<div class="issue ${i.level}">${i.level === "bad" ? "● " : "⚠ "}${i.message}</div>`)
      .join("");
  });

  return <div ref={node => { el = node; }}></div>;
}

// --- helpers -------------------------------------------------------------

export function interestingIfaces(snap: Snapshot) {
  return snap.ifaces.filter((i) => i.active || i.down_total > 0 || i.up_total > 0);
}
