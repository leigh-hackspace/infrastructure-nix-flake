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
  leases?: {
    /** false when the router was started with `--no-leases` (no lease file). */
    enabled: boolean;
    /** the lease length dnsmasq hands out (`leaseTtl`), in seconds. */
    ttl: number;
    max_rows: number;
    /** every VLAN the router knows about, whether or not it has leases. */
    vlans: string[];
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

// --- the DHCP lease table (/api/leases) -----------------------------------

/** One lease, as dnsmasq's lease file has it plus what the address's block says. */
export interface LeaseRow {
  ip: string;
  /** what the client called itself; "" for dnsmasq's own `ra` rows. */
  name: string;
  /** the MAC, or `*` for a row dnsmasq generated rather than one a client took. */
  mac: string;
  /** dnsmasq's client-id: how the client was matched, and may be empty. */
  cid: string;
  /** wall-clock epoch seconds when this address is handed out again. */
  expires: number;
  /** the lease length the router hands out (`--lease-ttl`). */
  ttl: number;
  /** whether `ttl` describes *this* lease's block rather than the closest guess. */
  known: boolean;
  vlan: string | null;
  /** the network the address is in (`10.3.2.0/24`), or "". */
  net: string;
  /** seconds until it is handed out again; negative once it has passed. */
  left: number;
  /** percent of the lease still to run; null when the length is unknown. */
  pct: number | null;
  /** when it was issued, recovered from the expiry; null when unknown. */
  issued: number | null;
  /** dnsmasq's `ra-*` row rather than a client's lease. */
  generated: boolean;
}

export interface LeaseFilter {
  sort: "expires" | "issued" | "name" | "vlan" | "addr";
  /** "" keeps every lease; a number keeps only those expiring within it. */
  maxLeft: number;
  vlan: string;
  q: string;
}

export interface LeasesResponse {
  /** the router's clock when it answered: countdowns are relative to this. */
  now: number;
  lease_ttl: number;
  file: string;
  total: number;
  /** lease-file lines that were not leases (dnsmasq's own header, a torn write). */
  skipped: number;
  matched: number;
  returned: number;
  expired: number;
  /** leases whose block is not one the router was told about, so no true length. */
  unknown_ttl: number;
  generated: number;
  max_left: number;
  /** unix seconds when the lease file was last written, or null when the OS
   *  would not say. A fact about the *channel*, not about any lease. */
  source_mtime: number | null;
  /** seconds between that write and `now`; null when the mtime is unknown. */
  source_age: number | null;
  /** why this table cannot be the truth, if it cannot be: `"all-expired"` or
   *  `"unwritten"` — the backend's verdict (`leases.rs::stale`), null when it
   *  found nothing wrong with the channel. */
  stale: string | null;
  /** the count of leases per VLAN, keyed by VLAN device name. */
  vlans: Record<string, number>;
  /** the count of leases per network, keyed by prefix. */
  networks: Record<string, number>;
  filter: { max_left: number; vlan: string | null; net: string | null; sort: string; q: string | null; limit: number };
  defaults: { max_left: number; limit: number };
  vlan_labels: string[];
  leases: LeaseRow[];
}

export async function loadLeases(f: LeaseFilter): Promise<LeasesResponse> {
  const p = new URLSearchParams();
  p.set("sort", f.sort);
  if (f.maxLeft >= 0) p.set("max_left", String(f.maxLeft));
  if (f.vlan) p.set("vlan", f.vlan);
  if (f.q) p.set("q", f.q);
  return json<LeasesResponse>(await fetch(`/api/leases?${p}`, { cache: "no-store" }));
}
