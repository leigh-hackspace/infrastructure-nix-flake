//! The root component: the tabs and the one panel that is shown.
//!
//! The tabs are by **question**, not by widget. "What is the router doing right now" and
//! "what is on the connection table" are different questions, and answering the second one
//! means reading rows rather than shapes, so the page has a tab per question and the
//! overview is the summary of all of them. That is the shape the page has always had, and
//! the port keeps it because it is the reason the page is readable at all.
//!
//! The tab state and the URL fragment are the same fact, held in `state.rs`: the fragment
//! names the tab on load, `choose` writes it back, and the `hashchange` listener makes the
//! browser's own history drive the tab. So a bookmark of `#leases` opens on leases.
//!
//! The tabs themselves are the component library's (`common_ui::tabs`) — the roles, the
//! roving focus, the arrow-key movement, and the `data-state` the CSS keys on all belong to
//! the vendored primitive. `docs/component-library.md` is why that library is vendored
//! rather than depended on, and it is also why the *styling* is here: the primitives are
//! unstyled, so the sheet in `index.html` keys on what the primitive emits
//! (`data-state="active"`, `data-orientation`, `data-disabled`) and passes the classes
//! through the components' own `class` props rather than reaching for `[role=…]` selectors.
//!
//! Every panel is `TabContent`, which renders its children only for the selected tab — so a
//! chart in a hidden panel has no canvas to draw on, which is what `charts.rs` reads
//! `store.tab` for, and why the suite names the tab it wants to look at.

use common_ui::tabs::{TabContent, TabList, TabTrigger, Tabs};
use dioxus::prelude::*;

use crate::charts::{BwCard, ConnChart, LoadChart};
use crate::connections::ConnectionsPanel;
use crate::leases::LeasesPanel;
use crate::panels::{Banner, Header, IfaceCards, Issues};
use crate::state::{PANELS, Store, start};
use crate::wan::WanPanel;

#[component]
pub fn App() -> Element {
    let store = use_context_provider(Store::new);

    use_effect(move || {
        store.install_listeners();
        start(store);
    });

    // `Tabs` is controlled: its value is a `ReadSignal<Option<String>>`, so the store is
    // the source of truth and the memo is what makes the widget follow it rather than the
    // other way round.
    let value = ReadSignal::from(use_memo(move || Some((*store.tab.read()).clone())));
    let on_change = use_callback(move |v: String| choose(store, &v));

    rsx! {
        main {
            Header { store: store }
            Banner { store: store }
            Tabs {
                class: "tabs",
                value: value,
                horizontal: true,
                roving_loop: true,
                on_value_change: on_change,
                TabList {
                    class: "tablist",
                    for (i, (id, label)) in PANELS.iter().enumerate() {
                        TabTrigger {
                            class: "tab",
                            value: id.to_string(),
                            index: i,
                            "{label}"
                        }
                    }
                }
                for (i, (id, _)) in PANELS.iter().enumerate() {
                    TabContent {
                        class: "tabpanel",
                        value: id.to_string(),
                        index: i,
                        {panel(store, id)}
                    }
                }
            }
        }
    }
}

/// A tab change writes the fragment, so the tab survives a reload and the browser's
/// back/forward keeps working. `set_tab` is guarded on inequality so this does not come back
/// through the listener as a second change.
fn choose(store: Store, id: &str) {
    store.set_tab(id);
    if let Some(w) = web_sys::window() {
        let _ = w.location().set_hash(id);
    }
}

fn panel(store: Store, id: &str) -> Element {
    match id {
        "overview" => rsx! {
            section {
                class: "cards",
                div { class: "card", ConnChart { store: store } }
                div { class: "card", LoadChart { store: store } }
            }
            IfaceCards { store: store }
            Issues { store: store }
        },
        "connections" => {
            rsx! { ConnectionsPanel { store: store } }
        }
        "uplink" => {
            rsx! { WanPanel { store: store } }
        }
        "leases" => {
            rsx! { LeasesPanel { store: store } }
        }
        _ => {
            rsx! { section { class: "card wide", BwCard { store: store } } }
        }
    }
}
