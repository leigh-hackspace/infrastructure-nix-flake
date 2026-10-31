import { createEffect } from "solid-js";
import { fmtDur, fmtTime } from "./format";
import type { LeaseFilter, LeaseRow, LeasesResponse } from "./api";
import type { Store } from "./store";

/**
 * Escape a value for innerHTML.
 *
 * A lease is the most device-supplied thing on this page: the hostname is a string
 * the client chose, the MAC and client-id are strings it sent, and dnsmasq copied
 * all three into its file verbatim.  `connections.tsx` has the same function for
 * the same reason — this table does not get to interpolate them raw because the
 * connection table already learned that lesson.
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

const SORT_OPTIONS: [LeaseFilter["sort"], string][] = [
  ["expires", "expiring first"],
  ["issued", "most recently issued"],
  ["name", "name"],
  ["vlan", "VLAN"],
  ["addr", "address"],
];

const LEFT_OPTIONS: [number, string][] = [
  [-1, "every lease"],
  [3600, "expiring within an hour"],
  [6 * 3600, "expiring within 6 hours"],
  [24 * 3600, "expiring today"],
];

/** How long a lease has left, as a word rather than a number of seconds. */
function fmtLeft(secs: number): string {
  if (secs < 0) return `lapsed ${fmtDur(-secs)} ago`;
  if (secs < 60) return `${Math.max(0, Math.round(secs))} s`;
  return fmtDur(secs);
}

/**
 * Why the backend thinks this table cannot be the truth, in the page's words.
 *
 * The verdict is the backend's (`crates/network-status/src/leases.rs::stale`), so
 * the reason and the counts beside it cannot disagree; this only translates the
 * reason it chose.  An unknown reason still banners — a verdict this file does not
 * know is still a verdict that something is wrong with the channel.
 */
const STALE_WHY: Record<string, string> = {
  "all-expired":
    "every lease in it has already lapsed, and a DHCP server that is serving always has one that has not",
  unwritten:
    "it has not been written for longer than two of these leases last, and dnsmasq rewrites it on every lease event",
};

function staleBanner(data: LeasesResponse): string {
  if (!data.stale) return "";
  const why = STALE_WHY[data.stale] ?? `the router reported the lease file as ${data.stale}`;
  const age =
    data.source_age == null
      ? ""
      : `, and it was last written ${esc(fmtDur(data.source_age))} ago`;
  return (
    `<span class="pill bad">not live</span> <span class="warn-text">` +
    `this table is a snapshot, not the network: ${esc(why)}${age}. dnsmasq is not ` +
    `writing to ${esc(data.file)}, so what is missing from here is not evidence ` +
    `that a device is absent — the router still holds the real leases. ` +
    `<code>machines/gw/lease-reader.nix</code> is what owns that file.</span><br/>`
  );
}

/**
 * The DHCP lease table: what the router has promised to whom.
 *
 * The countdown is the point of the table.  A lease's expiry is the one number
 * here that says something will *change*: when it lapses the client renews, and a
 * device that has been moved can come back on a different VLAN.  So the column
 * shows how much is left and when it runs out, against the router's own clock
 * (`store.routerNow()`), not the browser's.
 */
export function LeasesPanel(s: { store: Store }) {
  let summaryEl: HTMLDivElement | undefined;
  let bodyEl: HTMLTableSectionElement | undefined;
  let noteEl: HTMLDivElement | undefined;

  createEffect(() => {
    const data = s.store.leases();
    const err = s.store.leasesError();
    const now = s.store.routerNow();

    if (!data) {
      if (summaryEl) {
        summaryEl.innerHTML = err
          ? `<span class="pill bad">no lease data</span> <span class="muted">${esc(err)}</span>`
          : `<span class="muted">reading dnsmasq's lease file…</span>`;
      }
      return;
    }

    if (summaryEl) {
      const truncated = data.matched > data.returned;
      const per = Object.entries(data.vlans)
        .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
        .map(([v, n]) => `${esc(v)} ${n}`)
        .join(" · ");
      summaryEl.innerHTML =
        staleBanner(data) +
        `<span class="mono">${data.total}</span> lease${data.total === 1 ? "" : "s"}` +
        (data.expired ? ` <span class="warn-text">(${data.expired} lapsed)</span>` : "") +
        (per ? ` <span class="muted">· ${per}</span>` : "") +
        (truncated ? ` <span class="muted">· showing ${data.returned} of ${data.matched}</span>` : "") +
        // Freshness in the healthy case too: "is this live?" is the first question
        // a lease table has to answer, and `not live` above is only half of it.
        (data.source_age == null
          ? ""
          : ` <span class="muted">· written ${esc(fmtDur(data.source_age))} ago</span>`) +
        ` <span class="muted">· ${esc(data.file)}</span>`;
    }

    if (bodyEl) {
      bodyEl.innerHTML = data.leases.length
        ? data.leases.map((l) => row(l, now)).join("")
        : `<tr><td colspan="5" class="muted empty">${emptyWhy(data, s.store.leaseControls())}</td></tr>`;
    }

    if (noteEl) {
      const why: string[] = [
        `dnsmasq's lease file records an expiry and no length, so \u201cleft\u201d is the block's own lease length (${esc(fmtDur(data.lease_ttl))}) counted back from that expiry: every lease here is ${esc(fmtDur(data.lease_ttl))} long because that is what this router hands out.`,
        `A client that renews keeps its address and the countdown restarts; a lease that has <em>lapsed</em> is still listed until the client asks again, which is why lapsed rows are shown rather than hidden.`,
      ];
      if (data.unknown_ttl > 0) {
        why.push(
          `${data.unknown_ttl} lease${data.unknown_ttl === 1 ? "" : "s"} ${data.unknown_ttl === 1 ? "is" : "are"} inside no block the router was told about (a tailnet or hand-written address), so the bar is dimmed: the length it actually ran at is not known here.`,
        );
      }
      if (data.generated > 0) {
        why.push(
          `${data.generated} row${data.generated === 1 ? "" : "s"} marked <span class="kind">ra</span> ${data.generated === 1 ? "is" : "are"} dnsmasq's own router-advertisement rows, not a client's lease — no MAC, no name, and no start time anywhere to count from.`,
        );
      }
      if (data.skipped > 0) {
        why.push(`${data.skipped} lease-file line${data.skipped === 1 ? "" : "s"} were not leases (dnsmasq's own header, or a write caught mid-flight) and were ignored.`);
      }
      noteEl.innerHTML = why.map((w) => `<div>${w}</div>`).join("");
    }
  });

  return (
    <>
      <div class="chart-head">
        <h2>DHCP leases</h2>
        <label class="ctl">
          show
          <select
            value={String(s.store.leaseControls().maxLeft)}
            onChange={(e) =>
              s.store.setLeaseControls({ maxLeft: Number((e.target as HTMLSelectElement).value) })
            }
          >
            {LEFT_OPTIONS.map(([v, l]) => <option value={String(v)}>{l}</option>)}
          </select>
        </label>
        <label class="ctl">
          sort
          <select
            value={s.store.leaseControls().sort}
            onChange={(e) =>
              s.store.setLeaseControls({ sort: (e.target as HTMLSelectElement).value as LeaseFilter["sort"] })
            }
          >
            {SORT_OPTIONS.map(([v, l]) => <option value={v}>{l}</option>)}
          </select>
        </label>
        <label class="ctl">
          VLAN
          <select
            value={s.store.leaseControls().vlan}
            onChange={(e) => s.store.setLeaseControls({ vlan: (e.target as HTMLSelectElement).value })}
          >
            <option value="">every VLAN</option>
            {/* The router's whole VLAN list, not just the ones with leases: a
                VLAN whose leases have all lapsed must still be selectable, or
                the filter would silently answer "nobody" instead of "nobody
                right now". */}
            {(s.store.config().leases?.vlans ?? []).map((v) => <option value={v}>{v}</option>)}
          </select>
        </label>
        <input
          class="ctl q"
          type="search"
          placeholder="name, address, MAC, VLAN…"
          value={s.store.leaseControls().q}
          onInput={(e) => s.store.setLeaseControls({ q: (e.target as HTMLInputElement).value })}
        />
      </div>

      <div class="conn-summary mono" ref={(el) => (summaryEl = el)}></div>

      <div class="conn-scroll">
        <table class="conn leases">
          <thead>
            <tr>
              <th>Device</th>
              <th>Address</th>
              <th>Where</th>
              <th class="num">Left</th>
              <th class="num">Expires</th>
            </tr>
          </thead>
          <tbody ref={(el) => (bodyEl = el)}></tbody>
        </table>
      </div>
      <div class="conn-note muted" ref={(el) => (noteEl = el)}></div>
    </>
  );
}

function row(l: LeaseRow, now: number): string {
  const left = l.expires - now;
  const name = l.name
    ? `${esc(l.name)}`
    : l.generated
      ? `<span class="kind" title="dnsmasq's own router-advertisement row, not a client's lease">ra</span>`
      : `<span class="muted">(no name)</span>`;
  // The MAC is the fallback identity, because it is the one field a client without
  // a hostname still has; `*` is dnsmasq's own marker and is shown as such rather
  // than as an address.
  const ident =
    l.mac && l.mac !== "*"
      ? `<span class="muted mono addr">${esc(l.mac)}</span>`
      : l.cid
        ? `<span class="muted mono addr">id ${esc(l.cid)}</span>`
        : "";
  const where = l.vlan
    ? `<span class="mono">${esc(l.vlan)}</span> <span class="muted mono addr">${esc(l.net)}</span>`
    : l.net
      ? `<span class="mono">${esc(l.net)}</span>`
      : `<span class="muted">not on a VLAN</span>`;
  // The bar is the share of the lease still to run, and it is absent — not empty —
  // when the length is unknown or the lease has already lapsed.
  const pct = l.known && left >= 0 && l.ttl > 0 ? Math.min(100, (left / l.ttl) * 100) : null;
  const bar =
    pct == null
      ? ""
      : `<span class="bar lease${pct < 15 ? " low" : ""}" style="width:${pct.toFixed(1)}%"></span>`;
  const cls = left < 0 ? "lapsed" : pct != null && pct < 15 ? "low" : "";
  return (
    `<tr class="${cls}">` +
    `<td>${name}${ident}</td>` +
    `<td class="mono">${esc(l.ip)}</td>` +
    `<td>${where}</td>` +
    `<td class="num">${bar}${esc(fmtLeft(left))}</td>` +
    `<td class="num muted">${esc(fmtTime(l.expires))}</td>` +
    `</tr>`
  );
}

function emptyWhy(data: LeasesResponse, c: { maxLeft: number; vlan: string; q: string }): string {
  if (data.total === 0) {
    return "the lease file is readable but empty: no client has been given an address since dnsmasq last started";
  }
  const filters = [
    c.maxLeft >= 0 ? `expiring within ${fmtDur(c.maxLeft)}` : null,
    c.vlan ? `on ${c.vlan}` : null,
    c.q ? `matching “${c.q}”` : null,
  ].filter(Boolean);
  return `${data.total} lease${data.total === 1 ? "" : "s"} in the file, none ${filters.join(" and ") || "matching"}`;
}
