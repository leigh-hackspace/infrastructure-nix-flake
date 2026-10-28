import { createEffect } from "solid-js";
import { fmtDur, fmtRate, fmtTotal } from "./format";
import type { ConnFilter, ConnKind, ConnRow, HostAgg } from "./api";
import type { Store } from "./store";

/**
 * Escape a value for innerHTML.
 *
 * Not paranoia: the strings in this table come from the kernel and from DHCP
 * hostnames, and a hostname is the one field on a router's connection table that
 * a device chooses for itself.  Interface names were safe to interpolate; this
 * table is not the same data source and must not inherit that habit.
 */
function esc(s: string | null | undefined): string {
  if (s == null) return "";
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

const KIND_LABEL: Record<ConnKind, string> = {
  out: "out",
  in: "in",
  lan: "LAN",
  wan: "other",
};

const AGE_OPTIONS: [number, string][] = [
  [-1, "server default"],
  [0, "no minimum"],
  [5, "5 s"],
  [15, "15 s"],
  [30, "30 s"],
  [60, "1 min"],
  [300, "5 min"],
];

const SORT_OPTIONS: [string, string][] = [
  ["rate", "busiest now"],
  ["down", "downloading"],
  ["up", "uploading"],
  ["bytes", "most transferred"],
  ["age", "oldest"],
  ["host", "device"],
];

const KIND_OPTIONS: [string, string][] = [
  ["", "every direction"],
  ["out", "device → internet"],
  ["in", "internet → device"],
  ["lan", "device ↔ device"],
  ["wan", "neither end is ours"],
];

/** The `--conn-min-age` question, in one line, next to the table it filters. */
export function ConnectionsPanel(s: { store: Store }) {
  let summaryEl: HTMLDivElement | undefined;
  let hostsEl: HTMLDivElement | undefined;
  let bodyEl: HTMLTableSectionElement | undefined;
  let noteEl: HTMLDivElement | undefined;

  createEffect(() => {
    const data = s.store.conns();
    const err = s.store.connsError();
    const controls = s.store.controls();

    if (!data) {
      if (summaryEl) {
        summaryEl.innerHTML = err
          ? `<span class="pill bad">no connection data</span> <span class="muted">${esc(err)} — the table will appear as soon as the router answers.</span>`
          : `<span class="muted">reading the connection table…</span>`;
      }
      return;
    }

    const t = data.totals;
    const truncated = data.matched > data.returned;
    if (summaryEl) {
      summaryEl.innerHTML =
        `<span class="downc">▼ ${esc(fmtRate(t.down_bps))}</span>` +
        ` &nbsp;<span class="upc">▲ ${esc(fmtRate(t.up_bps))}</span>` +
        ` <span class="muted">across ${data.active} live connection${data.active === 1 ? "" : "s"}` +
        `, ${data.matched} over ${esc(fmtDur(controls.minAge === -1 ? s.store.effectiveMinAge() : controls.minAge))}` +
        (truncated ? `, showing the top ${data.returned} of ${data.matched}` : "") +
        ` · sampled every ${data.interval}s</span>`;
    }

    // Top talkers, unfiltered by the age slider on purpose: "who is the biggest
    // talker" cannot depend on the filter you set to answer it.
    if (hostsEl) {
      const hosts = data.hosts.slice(0, 6);
      hostsEl.innerHTML = hosts.length
        ? hosts.map((h) => hostChip(h)).join("")
        : `<span class="muted">no device is moving anything right now</span>`;
    }

    if (bodyEl) {
      const max = Math.max(1, ...data.connections.map((c) => Math.max(c.down_bps ?? 0, c.up_bps ?? 0)));
      bodyEl.innerHTML = data.connections.length
        ? data.connections.map((c) => row(c, max)).join("")
        : `<tr><td colspan="7" class="muted empty">${emptyWhy(data, err)}</td></tr>`;
    }

    if (noteEl) {
      const why: string[] = [
        "Age is how long <em>this page</em> has seen the connection — conntrack records no start time — and rates are differenced between samples, so a connection needs two samples to have one.",
        `\u201cdown\u201d is always peer \u2192 device, whichever way the connection started. Addresses come from the router's DNS records and dnsmasq's lease file (the DHCP leases tab has the same rows, with what each client calls itself); anything in neither stays a number.`,
      ];
      if (data.unaccounted > 0) {
        why.push(
          `<span class="warn-text">${data.unaccounted} connection${data.unaccounted === 1 ? "" : "s"} the kernel is not counting bytes for` +
            ` (they began before net.netfilter.nf_conntrack_acct was on) — shown as \u2013, not as zero.</span>`,
        );
      }
      if (data.unparsed > 0) {
        why.push(`${data.unparsed} table rows would not parse and were dropped.`);
      }
      noteEl.innerHTML = why.map((w) => `<div>${w}</div>`).join("");
    }
  });

  return (
    <>
      <div class="chart-head">
        <h2>Active connections</h2>
        <label class="ctl">
          older than
          <select
            value={String(s.store.controls().minAge)}
            onChange={(e) => s.store.setControls({ minAge: Number((e.target as HTMLSelectElement).value) })}
          >
            {AGE_OPTIONS.map(([v, l]) => <option value={String(v)}>{l}</option>)}
          </select>
        </label>
        <label class="ctl">
          sort
          <select
            value={s.store.controls().sort}
            onChange={(e) =>
              s.store.setControls({
                sort: (e.target as HTMLSelectElement).value as ConnFilter["sort"],
              })
            }
          >
            {SORT_OPTIONS.map(([v, l]) => <option value={v}>{l}</option>)}
          </select>
        </label>
        <label class="ctl">
          <select
            value={s.store.controls().kind}
            onChange={(e) => s.store.setControls({ kind: (e.target as HTMLSelectElement).value as ConnKind | "" })}
          >
            {KIND_OPTIONS.map(([v, l]) => <option value={v}>{l}</option>)}
          </select>
        </label>
        <label class="ctl">
          <input
            type="checkbox"
            checked={s.store.controls().busyOnly}
            onChange={(e) => s.store.setControls({ busyOnly: (e.target as HTMLInputElement).checked })}
          />{" "}
          moving now
        </label>
        <input
          class="ctl q"
          type="search"
          placeholder="name, address, port…"
          value={s.store.controls().q}
          onInput={(e) => s.store.setControls({ q: (e.target as HTMLInputElement).value })}
        />
      </div>

      <div class="conn-summary mono" ref={(el) => (summaryEl = el)}></div>
      <div class="conn-hosts" ref={(el) => (hostsEl = el)}></div>

      <div class="conn-scroll">
        <table class="conn">
          <thead>
            <tr>
              <th>Device</th>
              <th>Peer</th>
              <th>Service</th>
              <th class="num">Age</th>
              <th class="num">Down</th>
              <th class="num">Up</th>
            </tr>
          </thead>
          <tbody ref={(el) => (bodyEl = el)}></tbody>
        </table>
      </div>
      <div class="conn-note muted" ref={(el) => (noteEl = el)}></div>
    </>
  );
}

function hostChip(h: HostAgg): string {
  const who = h.name ? esc(h.name) : esc(h.addr);
  const addr = h.name ? ` <span class="muted mono">${esc(h.addr)}</span>` : "";
  const more = h.conns > 1 ? ` <span class="muted">${h.conns}c</span>` : "";
  return (
    `<span class="chip"><span class="who">${who}</span>${addr}${more}` +
    ` <span class="downc">▼ ${esc(fmtRate(h.down_bps))}</span>` +
    ` <span class="upc">▲ ${esc(fmtRate(h.up_bps))}</span></span>`
  );
}

function row(c: ConnRow, max: number): string {
  const dev = c.host_name
    ? `${esc(c.host_name)} <span class="muted mono addr">${esc(c.host)}${c.host_port ? ":" + c.host_port : ""}</span>`
    : `<span class="mono">${esc(c.host)}${c.host_port ? ":" + c.host_port : ""}</span>`;
  const peer = c.peer_name
    ? `${esc(c.peer_name)} <span class="muted mono addr">${esc(c.peer)}</span>`
    : `<span class="mono">${esc(c.peer)}${c.peer_port ? ":" + c.peer_port : ""}</span>`;
  const svc = [c.service || c.proto, c.state && c.state !== "ESTABLISHED" ? c.state : null]
    .filter(Boolean)
    .map((x) => esc(String(x)))
    .join(' <span class="muted">·</span> ');
  const kind = `<span class="kind k-${esc(c.kind)}" title="${esc(c.kind)} connection">${KIND_LABEL[c.kind]}</span>`;
  const bytes = c.accounted
    ? `<span class="muted">${esc(fmtTotal(c.bytes_down))}</span>`
    : `<span class="muted" title="the kernel counted no bytes for this connection (it began before conntrack accounting was switched on)">–</span>`;
  const upBytes = c.accounted
    ? `<span class="muted">${esc(fmtTotal(c.bytes_up))}</span>`
    : `<span class="muted">–</span>`;

  return (
    `<tr class="kind-${esc(c.kind)}${c.accounted ? "" : " uncounted"}">` +
    `<td>${kind} ${dev}</td>` +
    `<td>${peer}</td>` +
    `<td>${svc}</td>` +
    `<td class="num muted">${esc(fmtDur(c.age))}</td>` +
    `<td class="num">${bar(c.down_bps, max, "down")}${esc(fmtRate(c.down_bps))}<br/>${bytes}</td>` +
    `<td class="num">${bar(c.up_bps, max, "up")}${esc(fmtRate(c.up_bps))}<br/>${upBytes}</td>` +
    `</tr>`
  );
}

/** A rate cell's background is its share of the busiest row, so the table can
 *  be scanned rather than read. */
function bar(bps: number | null, max: number, cls: string): string {
  if (!bps || bps <= 0) return "";
  const pct = Math.min(100, (bps / max) * 100);
  return `<span class="bar ${cls}" style="width:${pct.toFixed(1)}%"></span>`;
}

function emptyWhy(data: { active: number; matched: number; tracked: number }, err: string | null): string {
  if (err) return `the connection table could not be read: ${esc(err)}`;
  if (data.active === 0) return "no connections in the router's table right now";
  return (
    `${data.active} live connection${data.active === 1 ? "" : "s"}, none matching the filter` +
    ` (${data.tracked} tracked). Short connections come and go: try a lower minimum age.`
  );
}
