import { createEffect } from "solid-js";
import { useStore } from "./store";
import { Header, Banner, IfaceCards, Issues, interestingIfaces } from "./panels";
import { BwCard, ConnChart, LoadChart } from "./charts";

export function App() {
  const store = useStore();
  store.start();

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

      <section class="cards">
        <div class="card wide">
          <BwCard
            store={store}
            wan={() => store.config().wan}
            setSelectedIface={store.setSelectedIface}
          />
        </div>
        <div class="card">
          <h2>Connections (firewall state table)</h2>
          <ConnChart store={store} />
        </div>
        <div class="card">
          <h2>Load average</h2>
          <LoadChart store={store} />
        </div>
      </section>

      <section class="card" style="margin-bottom:16px">
        <h2>Interfaces</h2>
        <IfaceCards store={store} />
      </section>

      <section class="card">
        <h2>Issues</h2>
        <Issues store={store} />
      </section>
    </main>
  );
}
