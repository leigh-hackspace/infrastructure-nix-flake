import { createEffect } from "solid-js";
import { drawChart } from "./chart";
import { fmtRate, fmtTotal } from "./format";
import type { History, Store } from "./store";
import type { Snapshot } from "./api";
import { interestingIfaces } from "./panels";

/** Sum of up/down across every interface, right-aligned to the history. */
function totalSeries(hist: History) {
  const n = hist.ts.length;
  const up = new Array<number | null>(n).fill(null);
  const down = new Array<number | null>(n).fill(null);
  for (const name in hist.rates) {
    const r = hist.rates[name];
    const off = n - r.up.length;
    for (let i = 0; i < r.up.length; i++) {
      const uv = r.up[i], dv = r.down[i];
      if (uv != null) up[off + i] = (up[off + i] ?? 0) + uv;
      if (dv != null) down[off + i] = (down[off + i] ?? 0) + dv;
    }
  }
  return { up, down };
}

function optionList(snap: Snapshot | null, wan: string) {
  const opts = [{ name: "total", label: "total (all interfaces)" }];
  for (const i of snap ? interestingIfaces(snap) : []) {
    opts.push({ name: i.name, label: i.name === wan ? i.name + " (WAN)" : i.name });
  }
  return opts;
}

export function BwCard(s: { store: Store } & { wan: () => string; setSelectedIface: (v: string) => void }) {
  let canvas: HTMLCanvasElement | undefined;
  let nowEl: HTMLElement | undefined;
  let totalEl: HTMLElement | undefined;
  let sel: HTMLSelectElement | undefined;

  createEffect(() => {
    const hist = s.store.hist();
    s.store.redraw();
    const selected = s.store.selectedIface();
    const snap = s.store.last();
    const wan = s.wan();

    let upSeries = [0 as number | null];
    let downSeries = [0 as number | null];
    let curUp: number | null = null;
    let curDown: number | null = null;
    let totUp = 0;
    let totDown = 0;

    if (selected === "total") {
      const t = totalSeries(hist);
      upSeries = t.up;
      downSeries = t.down;
      if (snap) {
        for (const i of snap.ifaces) {
          if (i.up_bps != null) curUp = (curUp ?? 0) + i.up_bps;
          if (i.down_bps != null) curDown = (curDown ?? 0) + i.down_bps;
          totUp += i.up_total;
          totDown += i.down_total;
        }
      }
    } else {
      const h = hist.rates[selected];
      if (h) {
        upSeries = h.up;
        downSeries = h.down;
        const cur = snap?.ifaces.find((i) => i.name === selected);
        curUp = cur ? cur.up_bps : null;
        curDown = cur ? cur.down_bps : null;
        if (cur) {
          totUp = cur.up_total;
          totDown = cur.down_total;
        }
      }
    }

    if (canvas) {
      drawChart(canvas, [
        { data: upSeries, color: "#2dd4bf", fill: true, window: 120 },
        { data: downSeries, color: "#60a5fa", fill: true, window: 120 },
      ], {
        fmt: fmtRate,
        height: 220,
        ts: hist.ts,
      });
    }

    if (nowEl) nowEl.innerHTML =
      `<span class="upc">▲ ${fmtRate(curUp)}</span>&nbsp; <span class="downc">▼ ${fmtRate(curDown)}</span>`;
    if (totalEl) totalEl.textContent =
      `${selected === "total" ? "all: " : ""}▼ ${fmtTotal(totDown)} · ▲ ${fmtTotal(totUp)}`;

    // Keep the interface selector in sync.
    if (sel) {
      const opts = optionList(snap, wan);
      if (sel.innerHTML !== (opts.map((o) => o.name).join(","))) {
        sel.innerHTML = opts
          .map((o) => `<option value="${o.name}"${o.name === selected ? " selected" : ""}>${o.label}</option>`)
          .join("");
      }
      sel.value = selected;
    }
  });

  // Non-reactive body: reads no signals, so this runs once and the refs above
  // stay valid for the life of the component.
  return (
    <div>
      <div class="chart-head">
        <h2>Bandwidth</h2>
        <select ref={el => { sel = el; }} onchange={() => s.setSelectedIface(sel ? sel.value : "total")}></select>
        <span class="now mono" ref={el => { nowEl = el; }}></span>
      </div>
      <canvas ref={el => { canvas = el; }} />
      <div class="legend">
        <span><span class="sw" style="background:var(--up)"></span>up (to internet)</span>
        <span><span class="sw" style="background:var(--down)"></span>down (from internet)</span>
        <span class="muted" ref={el => { totalEl = el; }}></span>
      </div>
    </div>
  );
}

export function ConnChart(s: { store: Store }) {
  let canvas: HTMLCanvasElement | undefined;
  let nowEl: HTMLElement | undefined;

  createEffect(() => {
    const hist = s.store.hist();
    s.store.redraw();
    const snap = s.store.last();
    if (canvas) {
      drawChart(canvas, [{ data: hist.pf, color: "#a78bfa", fill: true, window: 240 }], {
        fmt: (v) => Math.round(v) + "",
        height: 150,
        ts: hist.ts,
      });
    }
    if (nowEl) nowEl.textContent =
      `${snap?.router.pf_states != null ? snap!.router.pf_states + " active states" : "–"}` +
      `${snap?.router.own_tcp != null ? "  ·  " + snap!.router.own_tcp + " router sockets" : ""}` +
      `${snap?.router.retrans_rate != null ? "  ·  retrans " + snap!.router.retrans_rate.toFixed(2) + "/s" : ""}`;
  });

  return (
    <div>
      <canvas ref={el => { canvas = el; }} />
      <div class="legend"><span class="mono" ref={el => { nowEl = el; }}></span></div>
    </div>
  );
}

export function LoadChart(s: { store: Store }) {
  let canvas: HTMLCanvasElement | undefined;
  let nowEl: HTMLElement | undefined;

  createEffect(() => {
    const hist = s.store.hist();
    s.store.redraw();
    const snap = s.store.last();
    if (canvas) {
      drawChart(canvas, [{ data: hist.load, color: "#f1c40f", fill: true, window: 240 }], {
        fmt: (v) => v.toFixed(1),
        height: 150,
        ts: hist.ts,
      });
    }
    if (nowEl) nowEl.textContent =
      snap?.router.load ? `load ${snap!.router.load.map((l) => l.toFixed(2)).join(" / ")}` : "–";
  });

  return (
    <div>
      <canvas ref={el => { canvas = el; }} />
      <div class="legend"><span class="mono" ref={el => { nowEl = el; }}></span></div>
    </div>
  );
}
