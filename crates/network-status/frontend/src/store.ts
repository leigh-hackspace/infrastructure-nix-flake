import { createSignal } from "solid-js";
import {
  loadConfig,
  loadConnections,
  loadHistory,
  loadSnapshot,
  type AppConfig,
  type ConnFilter,
  type ConnKind,
  type ConnectionsResponse,
  type HistorySeries,
  type Snapshot,
} from "./api";

export interface History {
  ts: number[];
  rates: Record<string, { up: (number | null)[]; down: (number | null)[] }>;
  conntrack: (number | null)[];
  load: (number | null)[];
}

const MAXPTS = 720; // ~1 hour of 5s samples on the client

/**
 * The connection table's controls.  `minAge: -1` means "whatever the backend
 * was started with" (`--conn-min-age`), which is what lets the page show the
 * operator's default without hard-coding it here.
 */
export interface ConnControls {
  minAge: number;
  sort: ConnFilter["sort"];
  q: string;
  kind: ConnKind | "";
  /** Hide connections with nothing moving right now (rate 0 in both ways). */
  busyOnly: boolean;
}

export interface Store {
  hist: () => History;
  last: () => Snapshot | null;
  config: () => AppConfig;
  selectedIface: () => string;
  setSelectedIface: (v: string) => void;
  redraw: () => number;
  start: () => void;

  conns: () => ConnectionsResponse | null;
  /** Why the connection table is not showing data, if it is not. */
  connsError: () => string | null;
  controls: () => ConnControls;
  setControls: (patch: Partial<ConnControls>) => void;
  /** The age filter actually in force: the chosen one, or the server's. */
  effectiveMinAge: () => number;
}

export function useStore(): Store {
  const [hist, setHist] = createSignal<History>({ ts: [], rates: {}, conntrack: [], load: [] });
  const [last, setLast] = createSignal<Snapshot | null>(null);
  const [config, setConfig] = createSignal<AppConfig>({ wan: "ppp0", title: "network-info" });
  const [selectedIface, setSelectedIface] = createSignal<string>("total");
  const [redraw, setRedraw] = createSignal(0);
  const [conns, setConns] = createSignal<ConnectionsResponse | null>(null);
  const [connsError, setConnsError] = createSignal<string | null>(null);
  const [controls, setControlsSig] = createSignal<ConnControls>({
    minAge: -1,
    sort: "rate",
    q: "",
    kind: "",
    busyOnly: false,
  });

  let lastTs: number | null = null;

  // Push one sample into the rolling history (right-aligned sums, so an
  // interface that appears later does not shift the totals).
  function pushPoint(p: {
    ts: number;
    ifaces: { name: string; up_bps: number | null; down_bps: number | null }[];
    conntrack: number | null;
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
      const conntrack = [...h.conntrack, p.conntrack == null ? null : p.conntrack];
      const load = [...h.load, p.load ? p.load[0] : null];
      while (ts.length > MAXPTS) {
        ts.shift();
        conntrack.shift();
        load.shift();
        for (const k in rates) {
          rates[k].up.shift();
          rates[k].down.shift();
        }
      }
      return { ts, rates, conntrack, load };
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
        conntrack: h.conntrack[i] == null ? null : h.conntrack[i],
        load: h.load[i] == null ? null : [h.load[i] as number],
      });
    }
  }

  function effectiveMinAge(): number {
    const c = controls();
    if (c.minAge >= 0) return c.minAge;
    return config().connections?.min_age ?? 5;
  }

  function poll(): void {
    loadSnapshot()
      .then((res) => {
        if (!res.ok || res.error) return;
        res.wan = config().wan;
        pushPoint({
          ts: res.ts,
          ifaces: res.ifaces,
          conntrack: res.router.conntrack,
          load: res.router.load ?? null,
        });
        setLast(res);
      })
      .catch(() => undefined);
  }

  // The connection table is a second endpoint and can fail on its own (a router
  // started with --no-conns, a 500, a timeout), so it carries its own error
  // into the panel rather than leaving a table of nothing where an answer was.
  //
  // `wanted` is what makes the newest response win: the search box refetches on
  // every keystroke, and an older answer arriving after a newer one would put the
  // table back to what was typed before it.
  let wanted = 0;
  function pollConns(): void {
    if (config().connections && config().connections?.enabled === false) {
      setConnsError("this router was started with --no-conns");
      return;
    }
    const c = controls();
    const ticket = ++wanted;
    loadConnections({
      min_age: effectiveMinAge(),
      // "busy only" is a rate filter, so it is the backend's to apply: it is
      // applied before the row limit, and filtering here would let the first
      // hundred idle rows hide the traffic that is actually moving.
      min_bps: c.busyOnly ? 1 : 0,
      min_bytes: 0,
      kind: c.kind,
      sort: c.sort,
      q: c.q,
      limit: config().connections?.max_rows ?? 200,
    })
      .then((res) => {
        if (ticket !== wanted) return;
        setConns(res);
        setConnsError(null);
      })
      .catch((e: unknown) => {
        if (ticket !== wanted) return;
        setConnsError(e instanceof Error ? e.message : String(e));
      });
  }

  function setControls(patch: Partial<ConnControls>): void {
    setControlsSig((c) => ({ ...c, ...patch }));
    pollConns();
  }

  function start(): void {
    const onResize = () => setRedraw((r) => r + 1);
    window.addEventListener("resize", onResize);
    loadConfig().then((c) => {
      setConfig(c);
      // The search box's own default is the server's min-age; keep the two in
      // step rather than inventing a number here.
      pollConns();
    });
    loadHistory()
      .then(seedFromHistory)
      .catch(() => undefined)
      .finally(() => {
        poll();
        const secs = config().interval ?? 5;
        setInterval(() => {
          poll();
          pollConns();
        }, Math.max(1, secs) * 1000);
      });
  }

  return {
    hist,
    last,
    config,
    selectedIface,
    setSelectedIface,
    redraw,
    start,
    conns,
    connsError,
    controls,
    setControls,
    effectiveMinAge,
  };
}
