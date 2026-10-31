//! Shared state: the store's signals, the poll that fills them, and the two listeners.
//!
//! The rule this page has always had, and the one the port keeps: **a failed fetch keeps
//! the last data and carries its error into the panel that failed**. One endpoint can
//! refuse on its own — the lease file is group-read only (`machines/gw/lease-reader.nix`),
//! a router started with `--no-conns` answers 400 — and "no DHCP clients" is a conclusion
//! an operator would act on, so an error is shown rather than an empty table.
//!
//! Every field is a signal because that is what makes a panel redraw: `rsx!` re-renders
//! the part that *read* the signal that changed. The poll and the event handlers are the
//! only writers.

use std::time::Duration;

use dioxus::prelude::*;
use gloo_timers::future::sleep;
use js_sys::Date;
use wasm_bindgen::prelude::*;

use crate::api::{self, Config, ConnResponse, History, LeaseResponse, Snapshot};

/// Rolling window, in samples. The old number, kept: the sparklines draw 90 points and
/// the main chart 120, so 300 keeps both fed without a scrollback the page cannot draw.
pub const MAXPTS: usize = 300;

/// Fallback when `/api/config` has not answered — the same pair the old `loadConfig`
/// catch returned, so a page that cannot reach the API still says what it is.
pub fn default_config() -> Config {
    Config {
        wan: "ppp0".to_string(),
        title: "network-info".to_string(),
        interval: Some(5),
        connections: None,
        leases: None,
    }
}

/// The tabs, in order. `id` is what the URL fragment names — a bookmark of `#leases`
/// opens on leases — and `label` is what the tab prints. The tabs are by *question*, not
/// by widget; the reason is in `app.rs`.
pub const PANELS: [(&str, &str); 4] = [
    ("overview", "Overview"),
    ("connections", "Connections"),
    ("leases", "DHCP leases"),
    ("network", "Interfaces & issues"),
];

/// One sample, as the history window holds it. `Snapshot` and the wire `History` row both
/// feed it, which is why it is its own type rather than a fabricated snapshot.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Point {
    pub ts: i64,
    /// (interface name, up_bps, down_bps)
    pub ifaces: Vec<(String, Option<f64>, Option<f64>)>,
    pub conntrack: Option<u64>,
    /// First load value only — the chart plots load1.
    pub load: Option<f64>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ConnControls {
    /// −1 means "the server's default", which is what the panel prints.
    pub min_age: i64,
    pub min_bytes: i64,
    /// "moving now". This is the backend's filter, not the page's: it is applied before the
    /// row limit, so filtering here would let the first hundred idle rows hide the traffic
    /// that is actually moving — which is why it is a control and `api::conn_url` turns it
    /// into `min_bps` on the request rather than a stored value that could disagree with it.
    pub kind: String,
    pub sort: String,
    pub q: String,
    pub limit: usize,
    pub busy_only: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct LeaseControls {
    pub sort: String,
    /// −1 keeps every lease; a number keeps only those expiring within it.
    pub max_left: i64,
    pub vlan: String,
    pub q: String,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Store {
    pub hist: Signal<History>,
    pub last: Signal<Option<Snapshot>>,
    pub config: Signal<Config>,
    /// "total" or an interface name.
    pub selected: Signal<String>,
    pub conns: Signal<Option<ConnResponse>>,
    pub conns_err: Signal<Option<String>>,
    pub leases: Signal<Option<LeaseResponse>>,
    pub leases_err: Signal<Option<String>>,
    /// The snapshot's own failure, either the `error` the backend reported or the reason
    /// the request did not answer. The banner reads this.
    pub snap_err: Signal<Option<String>>,
    pub controls: Signal<ConnControls>,
    pub lease_controls: Signal<LeaseControls>,
    pub tab: Signal<String>,
    /// Bumped by the window `resize` listener: a canvas not redrawn after a resize keeps
    /// the picture it had at the old size.
    pub redraw: Signal<u32>,
    /// Bumped every poll, so a countdown redraws between responses. `router_now` reads it
    /// for exactly that reason — the value is the dependency, not the answer.
    pub tick: Signal<u64>,
    /// Router clock − browser clock, taken from a lease response's `now`, so every
    /// countdown is the router's arithmetic rather than the browser's.
    pub clock_skew: Signal<i64>,
    /// Newest-write-wins tickets: the search box refetches on every keystroke, and an
    /// older answer arriving after a newer one would put the table back to what was typed
    /// before it.
    pub wanted: Signal<u64>,
    pub wanted_leases: Signal<u64>,
    /// Guards so a sample is pushed once and in order.
    pub last_ts: Signal<i64>,
}

impl Store {
    pub fn new() -> Self {
        Self {
            hist: Signal::new(History::default()),
            last: Signal::new(None),
            config: Signal::new(default_config()),
            selected: Signal::new("total".to_string()),
            conns: Signal::new(None),
            conns_err: Signal::new(None),
            leases: Signal::new(None),
            leases_err: Signal::new(None),
            snap_err: Signal::new(None),
            controls: Signal::new(ConnControls {
                min_age: -1,
                min_bytes: 0,
                kind: String::new(),
                sort: "rate".to_string(),
                q: String::new(),
                limit: 200,
                busy_only: false,
            }),
            lease_controls: Signal::new(LeaseControls {
                sort: "expires".to_string(),
                max_left: -1,
                vlan: String::new(),
                q: String::new(),
            }),
            tab: Signal::new("overview".to_string()),
            redraw: Signal::new(0),
            tick: Signal::new(0),
            clock_skew: Signal::new(0),
            wanted: Signal::new(0),
            wanted_leases: Signal::new(0),
            last_ts: Signal::new(-1),
        }
    }

    /// The age the connection request actually used — the server's default when the
    /// control says "server default", so the panel can print the number it filtered by.
    pub fn effective_min_age(&self) -> i64 {
        let c = self.controls.read();
        if c.min_age >= 0 {
            return c.min_age;
        }
        self.config
            .read()
            .connections
            .as_ref()
            .map(|x| x.min_age)
            .unwrap_or(5)
    }

    pub fn router_now(&self) -> i64 {
        // Reading `tick` is the dependency that redraws a countdown between responses, so
        // the table does not freeze at "8h 12m" for a lease that has 8h 11m left.
        self.tick.read();
        (Date::now() / 1000.0) as i64 + *self.clock_skew.peek()
    }

    /// Push one sample into the rolling window. Right-aligned, so an interface that
    /// appears later does not shift the totals.
    pub fn push(mut self, p: &Point) {
        if *self.last_ts.peek() >= 0 && p.ts <= *self.last_ts.peek() {
            return;
        }
        self.last_ts.set(p.ts);

        let mut h = self.hist.write();
        for (name, up, down) in &p.ifaces {
            let e = h.ifaces.entry(name.clone()).or_default();
            e.up.push(*up);
            e.down.push(*down);
        }
        h.ts.push(p.ts);
        h.conntrack.push(p.conntrack);
        h.load.push(p.load);
        while h.ts.len() > MAXPTS {
            h.ts.remove(0);
            h.conntrack.remove(0);
            h.load.remove(0);
            for e in h.ifaces.values_mut() {
                e.up.remove(0);
                e.down.remove(0);
            }
        }
    }

    /// Seed the window from `/api/history` so a page load does not start with an empty
    /// chart that fills up over the next five minutes.
    pub fn seed(self, h: &History) {
        let mut base = i64::MIN;
        for (i, &ts) in h.ts.iter().enumerate() {
            if ts <= base {
                continue;
            }
            base = ts;
            let p = Point {
                ts,
                ifaces: h
                    .ifaces
                    .iter()
                    .map(|(name, s)| {
                        (
                            name.clone(),
                            s.up.get(i).copied().flatten(),
                            s.down.get(i).copied().flatten(),
                        )
                    })
                    .collect(),
                conntrack: h.conntrack.get(i).copied().flatten(),
                load: h.load.get(i).copied().flatten(),
            };
            self.push(&p);
        }
    }

    // ------------------------------------------------------------- fetches

    pub async fn poll(mut self) {
        match api::get_json::<Snapshot>("/api/snapshot").await {
            Ok(s) => {
                // The old store returned early on `!s.ok`, which made the banner dead
                // code: the snapshot it kept was by construction a good one. A snapshot
                // that reports a probe failure *is* the last known data plus the reason,
                // so it is kept — and the point is only pushed when the probe measured
                // something.
                if s.ok && s.error.is_none() {
                    let p = Point {
                        ts: s.ts,
                        ifaces: s
                            .ifaces
                            .iter()
                            .map(|i| (i.name.clone(), i.up_bps, i.down_bps))
                            .collect(),
                        conntrack: s.router.conntrack,
                        load: s.router.load.as_ref().map(|l| l[0]),
                    };
                    self.push(&p);
                }
                self.snap_err.set(s.error.clone());
                self.last.set(Some(s));
            }
            Err(e) => self.snap_err.set(Some(e)),
        }
    }

    pub async fn poll_conns(mut self) {
        if self
            .config
            .peek()
            .connections
            .as_ref()
            .map(|c| !c.enabled)
            .unwrap_or(false)
        {
            self.conns_err.set(Some("this router was started with --no-conns".to_string()));
            return;
        }
        let url = api::conn_url(&self.controls.read());
        let ticket = *self.wanted.peek() + 1;
        self.wanted.set(ticket);
        match api::get_json::<ConnResponse>(&url).await {
            Ok(r) => {
                if ticket != *self.wanted.peek() {
                    return;
                }
                self.conns.set(Some(r));
                self.conns_err.set(None);
            }
            Err(e) => {
                if ticket != *self.wanted.peek() {
                    return;
                }
                self.conns_err.set(Some(e));
            }
        }
    }

    pub async fn poll_leases(mut self) {
        if self
            .config
            .peek()
            .leases
            .as_ref()
            .map(|l| !l.enabled)
            .unwrap_or(false)
        {
            self.leases_err
                .set(Some("this router has no lease file configured (started with --no-leases)".to_string()));
            return;
        }
let url = api::lease_url(&self.lease_controls.read());
        let ticket = *self.wanted_leases.peek() + 1;
        self.wanted_leases.set(ticket);
        match api::get_json::<LeaseResponse>(&url).await {
            Ok(r) => {
                if ticket != *self.wanted_leases.peek() {
                    return;
                }
                // Keep the offset, not the number: the response's `now` is the router's
                // clock at the moment it answered.
                self.clock_skew.set(r.now - (Date::now() / 1000.0) as i64);
                self.leases.set(Some(r));
                self.leases_err.set(None);
            }
            Err(e) => {
                if ticket != *self.wanted_leases.peek() {
                    return;
                }
                self.leases_err.set(Some(e));
            }
        }
    }

    // -------------------------------------------------- control edits

    /// Change a connection control and refetch: the search box refetches per keystroke,
    /// which is what the ticket guard is for.
    pub fn edit_controls(mut self, f: impl FnOnce(&mut ConnControls)) {
        f(&mut self.controls.write());
        let store = self;
        spawn(async move { store.poll_conns().await; });
    }

    pub fn edit_lease_controls(mut self, f: impl FnOnce(&mut LeaseControls)) {
        f(&mut self.lease_controls.write());
        let store = self;
        spawn(async move { store.poll_leases().await; });
    }

    // ----------------------------------------------------- listeners

    /// `hashchange` and `resize`, installed once from the root component's effect.
    ///
    /// The fragment is the tab: assigning it is what makes the tab survive a reload, and
    /// the listener is what makes the browser's back/forward drive the tab. Writing the
    /// hash re-enters through the listener, so `set_tab` is guarded on inequality.
    ///
    /// The closures are leaked on purpose: a `Closure`'s Drop invalidates the JS function
    /// the listener holds, and these outlive the component that made them because the root
    /// component is the page. Same pattern as `switch-gui`'s popstate listener.
    pub fn install_listeners(self) {
        let Some(win) = web_sys::window() else { return };

        apply_hash(self, &win.location().hash().unwrap_or_default());

        let store = self;
        let hash: Closure<dyn FnMut()> = Closure::new(move || {
            let Some(w) = web_sys::window() else { return };
            apply_hash(store, &w.location().hash().unwrap_or_default());
        });
        let f: js_sys::Function = hash.as_js_value().clone().unchecked_into();
        let _ = win.add_event_listener_with_callback("hashchange", &f);
        std::mem::forget(hash);

        let mut store = self;
        let resize: Closure<dyn FnMut()> = Closure::new(move || {
            let n = *store.redraw.peek();
            store.redraw.set(n + 1);
        });
        let f: js_sys::Function = resize.as_js_value().clone().unchecked_into();
        let _ = win.add_event_listener_with_callback("resize", &f);
        std::mem::forget(resize);
    }

    pub fn set_tab(mut self, id: &str) {
        if *self.tab.peek() != id {
            self.tab.set(id.to_string());
        }
    }
}

fn apply_hash(store: Store, hash: &str) {
    let want = hash.trim_start_matches('#');
    if want.is_empty() || !PANELS.iter().any(|(id, _)| *id == want) {
        return;
    }
    store.set_tab(want);
}

/// The poll: config, then the history seed, then the loop.
///
/// `spawn_local`, not `spawn`: this loop is the page's lifetime, not one component's. A
/// plain `spawn` is cancelled when the component that made it is dropped, and a poll that
/// dies on a re-render stops updating a page that still looks alive — the same reason
/// `filestore`'s `js.rs` gives.
pub fn start(mut store: Store) {
    spawn(async move {
        match api::get_json::<Config>("/api/config").await {
            Ok(c) => {
                // The controls' defaults are the server's (`--conn-max-rows`), not this
                // page's guesses.
                let lim = c.connections.as_ref().map(|x| x.max_rows);
                store.config.set(c);
                if let Some(l) = lim {
                    store.controls.write().limit = l;
                }
            }
            Err(_) => store.config.set(default_config()),
        }

        if let Ok(h) = api::get_json::<History>("/api/history").await {
            store.seed(&h);
        }

        loop {
            store.poll().await;
            store.poll_conns().await;
            store.poll_leases().await;
            let n = *store.tick.peek();
            store.tick.set(n + 1);
            let secs = store.config.read().interval.unwrap_or(5).max(1);
            sleep(Duration::from_secs(secs)).await;
        }
    });
}
