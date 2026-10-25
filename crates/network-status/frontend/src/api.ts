// Types mirroring the JSON produced by the network-status backend.

export interface Iface {
  name: string;
  active: boolean;
  down_bps: number | null;
  up_bps: number | null;
  /** cumulative received bytes (from the router's perspective). */
  down_total: number;
  /** cumulative transmitted bytes. */
  up_total: number;
  errors: number;
}

export interface Issue {
  level: "bad" | "warn";
  message: string;
}

export interface Router {
  hostname: string;
  uptime_secs: number | null;
  load: [number, number, number] | null;
  ncpu: number | null;
  cpu: { user: number | null; system: number | null; idle: number | null };
  mem: { free: number | null; total: number | null };
  conntrack: number | null;
  own_tcp: number | null;
  retrans_rate: number | null;
}

export interface Snapshot {
  ts: number;
  ok: boolean;
  error: string | null;
  wan: string;
  router: Router;
  ifaces: Iface[];
  issues: Issue[];
}

export interface HistorySeries {
  ts: number[];
  ifaces: Record<string, { up: (number | null)[]; down: (number | null)[] }>;
  conntrack: (number | null)[];
  load: (number | null)[];
}

export interface AppConfig {
  /** interface name treated as the WAN for labels. */
  wan: string;
  title: string;
  /** seconds between the backend's samples; the page's rates are per interval. */
  interval?: number;
  connections?: {
    enabled: boolean;
    min_age: number;
    min_bytes: number;
    max_rows: number;
    stale: number;
    local_prefixes: string[];
  };
}

// --- the connection table (/api/connections) ------------------------------

/** Which side of the connection the network is on, as the backend labels it. */
export type ConnKind = "out" | "in" | "lan" | "wan";

export interface ConnRow {
  proto: string;
  /** TCP's state word; null for UDP/ICMP, which have none. */
  state: string | null;
  kind: ConnKind;
  assured: boolean;
  offloaded: boolean;
  /** The network's end: the device, not the direction. */
  host: string;
  host_name: string | null;
  host_port: number | null;
  /** The other end. */
  peer: string;
  peer_name: string | null;
  peer_port: number | null;
  /** A word for the port or ICMP type, when one is known. */
  service: string | null;
  /**
   * Seconds this dashboard has been seeing the flow — not the connection's age,
   * because conntrack does not record one (see the backend's `conntrack` module).
   */
  age: number;
  first_seen: number;
  samples: number;
  /** Peer→device and device→peer, since the connection began. Null when the */
  /** kernel was not counting it (created before the acct sysctl was on). */
  bytes_down: number | null;
  bytes_up: number | null;
  down_bps: number | null;
  up_bps: number | null;
  accounted: boolean;
  timeout: number;
}

/** A roll-up: one device's totals, or (as `totals`) the whole network's. */
export interface HostAgg {
  addr: string | null;
  name: string | null;
  conns: number;
  down_bps: number;
  up_bps: number;
  bytes_down: number;
  bytes_up: number;
  unaccounted: number;
}

export interface ConnFilter {
  min_age: number;
  min_bytes: number;
  min_bps: number;
  kind: ConnKind | "";
  sort: "rate" | "down" | "up" | "bytes" | "age" | "host";
  q: string;
  limit: number;
}

export interface ConnectionsResponse {
  ts: number;
  interval: number;
  tracked: number;
  active: number;
  matched: number;
  returned: number;
  unaccounted: number;
  unparsed: number;
  filter: ConnFilter;
  defaults: { min_age: number; min_bytes: number; limit: number };
  totals: HostAgg;
  hosts: HostAgg[];
  connections: ConnRow[];
}

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as T;
}

export async function loadConfig(): Promise<AppConfig> {
  try {
    return await json<AppConfig>(await fetch("/api/config", { cache: "no-store" }));
  } catch {
    return { wan: "ppp0", title: "network-info" };
  }
}

export async function loadHistory(): Promise<HistorySeries> {
  return json<HistorySeries>(await fetch("/api/history", { cache: "no-store" }));
}

export async function loadSnapshot(): Promise<Snapshot> {
  return json<Snapshot>(await fetch("/api/snapshot", { cache: "no-store" }));
}

/**
 * The connection table.  The filter is applied by the backend — the table is
 * thousands of rows and the page shows a hundred — so changing the age or the
 * search box costs one request, not a re-download of everything.
 */
export async function loadConnections(f: ConnFilter): Promise<ConnectionsResponse> {
  const p = new URLSearchParams();
  p.set("min_age", String(f.min_age));
  p.set("min_bytes", String(f.min_bytes));
  p.set("min_bps", String(f.min_bps));
  p.set("sort", f.sort);
  p.set("limit", String(f.limit));
  if (f.kind) p.set("kind", f.kind);
  if (f.q) p.set("q", f.q);
  return json<ConnectionsResponse>(await fetch(`/api/connections?${p}`, { cache: "no-store" }));
}
