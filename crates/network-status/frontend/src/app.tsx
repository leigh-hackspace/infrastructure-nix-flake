import { createEffect, createSignal, onCleanup } from "solid-js";
import { useStore } from "./store";
import { Header, Banner, IfaceCards, Issues, interestingIfaces } from "./panels";
import { BwCard, ConnChart, LoadChart } from "./charts";
import { ConnectionsPanel } from "./connections";
import { LeasesPanel } from "./leases";

/**
 * The page's tabs.
 *
 * The page used to be one column of cards — bandwidth, load, the connection
 * table, every interface, the issues — and it had got long enough that the answer
 * to "what is my laptop's lease" was a scroll past two tables and a chart grid.
 * The tabs are by *question*, not by widget: what is the router doing right now,
 * who is talking, what has the router promised to whom, and what is wrong.
 *
 * The choice lives in the URL fragment rather than in a signal, so a reload keeps
 * the tab and a link can point at one (`#leases`).  It is not a router — no
 * history push — because a tab that fills the browser's back stack is a tab that
 * makes "back" mean something it does not.
 */
const TABS = [
  { id: "overview", label: "Overview" },
  { id: "connections", label: "Connections" },
  { id: "leases", label: "DHCP leases" },
  { id: "network", label: "Interfaces & issues" },
] as const;

type TabId = (typeof TABS)[number]["id"];

function tabFromHash(): TabId {
  const h = (window.location.hash || "").replace(/^#/, "");
  return (TABS.find((t) => t.id === h)?.id ?? "overview") as TabId;
}

export function App() {
  const store = useStore();
  store.start();

  const [tab, setTab] = createSignal<TabId>(tabFromHash());
  const onHash = () => setTab(tabFromHash());
  window.addEventListener("hashchange", onHash);
  onCleanup(() => window.removeEventListener("hashchange", onHash));

  const choose = (id: TabId) => {
    // Assigning the fragment is what makes the tab survive a reload; the signal
    // is set directly too, so the page does not wait for the event to come back.
    if (window.location.hash !== `#${id}`) window.location.hash = id;
    setTab(id);
  };

  // Reset selection if the chosen interface has since disappeared.
  createEffect(() => {
    const snap = store.last();
    const sel = store.selectedIface();
    if (snap && sel !== "total" && !interestingIfaces(snap).some((i) => i.name === sel)) {
      store.setSelectedIface("total");
    }
  });

  return (
    <main>
      <Header store={store} />
      <Banner store={store} />

      <nav class="tabs" role="tablist">
        {TABS.map((t) => (
          <button
            type="button"
            role="tab"
            aria-selected={tab() === t.id}
            class={tab() === t.id ? "tab active" : "tab"}
            onClick={() => choose(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>

      {/* Every panel stays mounted; only the inactive ones are hidden.  The
          charts redraw on a canvas ref and the tables are rebuilt from a signal,
          so tearing a tab down and back up would empty a chart until the next
          sample and refetch a table for nothing. */}
      <section class="tabpanel cards" hidden={tab() !== "overview"}>
        <div class="card wide">
          <BwCard
            store={store}
            wan={() => store.config().wan}
            setSelectedIface={store.setSelectedIface}
          />
        </div>
        <div class="card">
          <h2>Connections (conntrack table)</h2>
          <ConnChart store={store} />
        </div>
        <div class="card">
          <h2>Load average</h2>
          <LoadChart store={store} />
        </div>
      </section>

      <section class="tabpanel card" hidden={tab() !== "connections"} style="margin-bottom:16px">
        <ConnectionsPanel store={store} />
      </section>

      <section class="tabpanel card" hidden={tab() !== "leases"} style="margin-bottom:16px">
        <LeasesPanel store={store} />
      </section>

      <section class="tabpanel" hidden={tab() !== "network"}>
        <div class="card" style="margin-bottom:16px">
          <h2>Interfaces</h2>
          <IfaceCards store={store} />
        </div>
        <div class="card">
          <h2>Issues</h2>
          <Issues store={store} />
        </div>
      </section>
    </main>
  );
}
