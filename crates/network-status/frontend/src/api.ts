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
  nprocs: number | null;
  cpu: { user: number | null; system: number | null; idle: number | null };
  mem: { free: number | null; active: number | null; wired: number | null };
  pf_states: number | null;
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
  pf: (number | null)[];
  load: (number | null)[];
}

export interface AppConfig {
  /** interface name treated as the WAN for labels. */
  wan: string;
  title: string;
}

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as T;
}

export async function loadConfig(): Promise<AppConfig> {
  try {
    return await json<AppConfig>(await fetch("/api/config", { cache: "no-store" }));
  } catch {
    return { wan: "em0", title: "network-info" };
  }
}

export async function loadHistory(): Promise<HistorySeries> {
  return json<HistorySeries>(await fetch("/api/history", { cache: "no-store" }));
}

export async function loadSnapshot(): Promise<Snapshot> {
  return json<Snapshot>(await fetch("/api/snapshot", { cache: "no-store" }));
}
