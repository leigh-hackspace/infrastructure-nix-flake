import { createSignal } from "solid-js";
import {
  loadConfig,
  loadConnections,
  loadHistory,
  loadLeases,
  loadSnapshot,
  type AppConfig,
  type ConnFilter,
  type ConnKind,
  type ConnectionsResponse,
  type HistorySeries,
  type LeaseFilter,
  type LeasesResponse,
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

/**
 * The lease table's controls.  `maxLeft: -1` keeps every lease, including ones
 * that have already lapsed and are waiting for the client's next renewal — which
 * is the interesting row when a device has the wrong address.
 */
export interface LeaseControls {
  sort: LeaseFilter["sort"];
  maxLeft: number;
  vlan: string;
  q: string;
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

  leases: () => LeasesResponse | null;
  leasesError: () => string | null;
  leaseControls: () => LeaseControls;
  setLeaseControls: (patch: Partial<LeaseControls>) => void;
  /**
   * Now, on the router's clock, in epoch seconds.
   *
   * The lease countdown is the only thing on this page that needs the time: a
   * browser whose clock is two hours fast would otherwise show every lease as
   * already expired.  So the offset between the two clocks is taken from the
   * `now` the lease response carries and applied here, and the countdown is the
   * router's arithmetic rather than the browser's.
   */
  routerNow: () => number;
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
  const [leases, setLeases] = createSignal<LeasesResponse | null>(null);
  const [leasesError, setLeasesError] = createSignal<string | null>(null);
  const [leaseControls, setLeaseControlsSig] = createSignal<LeaseControls>({
    sort: "expires",
    maxLeft: -1,
    vlan: "",
    q: "",
  });
  // Router clock minus browser clock, in seconds.  Refreshed by every lease
  // response; `routerNow()` is the only place either clock is read.
  let clockSkew = 0;
  const [tick, setTick] = createSignal(0);

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

  // The lease table is its own endpoint with its own failure: the lease file can
  // be unreadable (a router started without ./lease-reader.nix's permission, or a
  // dnsmasq that has just rewritten it), and that must read as an error in the
  // panel rather than as a table of nothing — "no DHCP clients" is a conclusion an
  // operator would act on.
  let wantedLeases = 0;
  function pollLeases(): void {
    const cfg = config().leases;
    if (cfg && cfg.enabled === false) {
      setLeasesError("this router has no lease file configured (started with --no-leases)");
      return;
    }
    const c = leaseControls();
    const ticket = ++wantedLeases;
    loadLeases({ sort: c.sort, maxLeft: c.maxLeft, vlan: c.vlan, q: c.q })
      .then((res) => {
        if (ticket !== wantedLeases) return;
        // Keep the *offset*, not the number: the response's `now` is corrected
        // for the round trip by being a whole second, and every countdown on the
        // page is then the router's arithmetic rather than the browser's clock.
        clockSkew = res.now - Date.now() / 1000;
        setLeases(res);
        setLeasesError(null);
      })
      .catch((e: unknown) => {
        if (ticket !== wantedLeases) return;
        setLeasesError(e instanceof Error ? e.message : String(e));
      });
  }

  function setLeaseControls(patch: Partial<LeaseControls>): void {
    setLeaseControlsSig((c) => ({ ...c, ...patch }));
    pollLeases();
  }

  function routerNow(): number {
    // Reading `tick` is deliberate: it is the dependency that redraws a countdown
    // between responses, so the table does not freeze at "8h 12m" for a lease
    // that has 8h 11m left.  `void` because the value is the dependency, not the
    // answer.
    void tick();
    return Date.now() / 1000 + clockSkew;
  }

  function start(): void {
    const onResize = () => setRedraw((r) => r + 1);
    window.addEventListener("resize", onResize);
    loadConfig().then((c) => {
      setConfig(c);
      // The search box's own default is the server's min-age; keep the two in
      // step rather than inventing a number here.
      pollConns();
      pollLeases();
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
          pollLeases();
          // Redraws the countdowns even when the lease rows are unchanged; see
          // `routerNow`.
          setTick((t) => t + 1);
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
    leases,
    leasesError,
    leaseControls,
    setLeaseControls,
    routerNow,
  };
}
