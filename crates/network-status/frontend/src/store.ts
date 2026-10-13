import { createSignal } from "solid-js";
import {
  loadConfig,
  loadHistory,
  loadSnapshot,
  type AppConfig,
  type HistorySeries,
  type Snapshot,
} from "./api";

export interface History {
  ts: number[];
  rates: Record<string, { up: (number | null)[]; down: (number | null)[] }>;
  pf: (number | null)[];
  load: (number | null)[];
}

const MAXPTS = 720; // ~1 hour of 5s samples on the client

export interface Store {
  hist: () => History;
  last: () => Snapshot | null;
  config: () => AppConfig;
  selectedIface: () => string;
  setSelectedIface: (v: string) => void;
  redraw: () => number;
  start: () => void;
}

export function useStore(): Store {
  const [hist, setHist] = createSignal<History>({ ts: [], rates: {}, pf: [], load: [] });
  const [last, setLast] = createSignal<Snapshot | null>(null);
  const [config, setConfig] = createSignal<AppConfig>({ wan: "em0", title: "network-info" });
  const [selectedIface, setSelectedIface] = createSignal<string>("total");
  const [redraw, setRedraw] = createSignal(0);

  let lastTs: number | null = null;

  // Push one sample into the rolling history (right-aligned sums, so an
  // interface that appears later does not shift the totals).
  function pushPoint(p: {
    ts: number;
    ifaces: { name: string; up_bps: number | null; down_bps: number | null }[];
    pf_states: number | null;
    load: number[] | null;
  }): void {
    if (lastTs !== null && p.ts <= lastTs) return;
    lastTs = p.ts;
    setHist((h) => {
      const rates: History["rates"] = { ...h.rates };
      for (const i of p.ifaces) {
        if (!rates[i.name]) rates[i.name] = { up: [], down: [] };
        rates[i.name].up.push(i.up_bps == null ? null : i.up_bps);
        rates[i.name].down.push(i.down_bps == null ? null : i.down_bps);
      }
      const ts = [...h.ts, p.ts];
      const pf = [...h.pf, p.pf_states == null ? null : p.pf_states];
      const load = [...h.load, p.load ? p.load[0] : null];
      while (ts.length > MAXPTS) {
        ts.shift();
        pf.shift();
        load.shift();
        for (const k in rates) {
          rates[k].up.shift();
          rates[k].down.shift();
        }
      }
      return { ts, rates, pf, load };
    });
  }

  function seedFromHistory(h: HistorySeries): void {
    let base = -Infinity;
    for (let i = 0; i < h.ts.length; i++) {
      if (h.ts[i] <= base) continue;
      base = h.ts[i];
      pushPoint({
        ts: h.ts[i],
        ifaces: Object.keys(h.ifaces).map((name) => ({
          name,
          up_bps: h.ifaces[name].up[i] == null ? null : h.ifaces[name].up[i],
          down_bps: h.ifaces[name].down[i] == null ? null : h.ifaces[name].down[i],
        })),
        pf_states: h.pf[i] == null ? null : h.pf[i],
        load: h.load[i] == null ? null : [h.load[i] as number],
      });
    }
  }

  function poll(): void {
    loadSnapshot()
      .then((res) => {
        if (!res.ok || res.error) return;
        res.wan = config().wan;
        pushPoint({
          ts: res.ts,
          ifaces: res.ifaces,
          pf_states: res.router.pf_states,
          load: res.router.load ?? null,
        });
        setLast(res);
      })
      .catch(() => undefined);
  }

  function start(): void {
    const onResize = () => setRedraw((r) => r + 1);
    window.addEventListener("resize", onResize);
    loadConfig().then((c) => setConfig(c));
    loadHistory()
      .then(seedFromHistory)
      .catch(() => undefined)
      .finally(() => {
        poll();
        setInterval(poll, 5000);
      });
  }

  return { hist, last, config, selectedIface, setSelectedIface, redraw, start };
}
