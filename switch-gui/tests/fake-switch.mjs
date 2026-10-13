// A fake ProCurve switch: the CLI's Telnet side, driven by an in-memory model.
//
// Exists so the web UI and the write queue can be tested without a device.  The
// real switches speak only Telnet, so there is no API to stub: what the tool
// talks to is a console, and the only honest double for a console is a process
// that behaves like one.  Everything in here is the behaviour the parsers and
// `Switch::command` actually depend on, taken from the fixtures in `src/parse.rs`
// (which are real captures) and from `switch-gui/README.md`:
//
//   * LF-only line endings, and the client sends a bare space for the key gate
//     and for `-- MORE --`, so the reader cannot wait on newlines alone;
//   * the 2824's banner ends with "Press any key to continue" and that keypress
//     is eaten, so it arrives before the login name;
//   * `Can't reach RADIUS server.` on a *successful* login (RADIUS refuses, the
//     local account database lets you in) — the note the page shows;
//   * a configuration command paints its echo and the next prompt on the **same
//     row**, which is why `is_prompt` matches a prompt as a suffix;
//   * `enable` is a second *login*, not a password prompt;
//   * nothing writes NVRAM, so the model is the whole truth and a reboot would
//     undo it — here, a reconnect does not, which is what makes the model
//     assertable between the queue's write and the sync's re-read.
//
// The HTTP side (GET /state, POST /reset, POST /fail) is the test's way of
// reading what the "device" believes, which is the only end of the pipeline that
// proves a write really went out.
//
// Run standalone: node fake-switch.mjs --telnet 22323 --state 22324 [--gate]

import net from "node:net";
import http from "node:http";

const LOGIN = "Please Enter Login Name: ";
const PASSWORD = "Please Enter Password: ";
const KEY_GATE = "Press any key to continue";
const RADIUS_NOISE = "Can't reach RADIUS server.";

/// The device the tests drive.  Port 23 is **shut and link-down**: it is the one
/// safe to write to, and the suite only ever touches ports like it.
export function defaultModel(name = "switch1") {
  const port = (o) => ({
    name: "",
    untagged: 1,
    tagged: [],
    link: "Up",
    shut: false,
    media: "100/1000T",
    trunk: null,
    ...o,
  });
  return {
    name,
    // Deliberately not one big flat set: a trunk member is locked in the GUI,
    // a shut port is the safe target, and a VLAN whose name has a space in it is
    // what the column parsers get wrong.
    vlans: [
      { id: 1, name: "DEFAULT" },
      { id: 225, name: "Shared" },
      { id: 228, name: "VLAN228" },
      { id: 232, name: "AD Lab" },
    ],
    ports: {
      1: port({ name: "GW Port 1", untagged: 1, tagged: [225] }),
      2: port({ name: "AP2", untagged: 225 }),
      3: port({ name: "Camera 3", untagged: 225, link: "Down" }),
      23: port({ untagged: 225, link: "Down", shut: true }),
      24: port({ name: "Spare 24", untagged: 225, link: "Down", shut: true }),
      47: port({ name: "Switch2 Port 47", untagged: 1, tagged: [225], trunk: "Trk1" }),
      48: port({ name: "Switch2 Port 48", untagged: 1, tagged: [225], trunk: "Trk1" }),
      Trk1: port({ name: "Uplink", untagged: 1, tagged: [225], media: "1000X-SFP" }),
    },
  };
}

/// Pad a column. The separating space is the literal one after each call in the
/// templates below, and it is not decoration.
///
/// Every table here is read by splitting on whitespace, so a value that exactly
/// fills its column glues itself to the next one — and `members()` reads a row
/// that is not four fields as the *end of the table*, so one glued column loses
/// every membership below it and the shadow copy quietly disagrees with the
/// device. The real CLI always has its space.
const pad = (s, w) => `${String(s).padEnd(w)}`;

/// `show vlan` — the 2510G shape (no `Voice` column).
function showVlan(m) {
  const rows = m.vlans
    .map((v) => `  ${pad(v.id, 7)} ${pad(v.name, 21)} ${pad("Port-based", 12)} No`)
    .join("\n");
  return ` Status and Counters - VLAN Information

  Maximum VLANs to support : 64
  Primary VLAN : DEFAULT
  Management VLAN :

  VLAN ID Name                 Status       Jumbo
  ------- -------------------- ------------ -----
${rows}
`;
}

/// `show vlan <id>` — membership.  `members()` splits on whitespace, so the
/// widths here are for the human reading the capture, not for the parser.
function showVlanPorts(m, id) {
  const v = m.vlans.find((x) => x.id === id);
  if (!v) return null;
  const rows = Object.entries(m.ports)
    .filter(([_, p]) => p.untagged === id || p.tagged.includes(id))
    .map(([name, p]) => {
      const mode = p.untagged === id ? "Untagged" : "Tagged";
      // `mode` must keep a trailing space: "Untagged" is exactly 8 characters,
      // and `members()` splits on whitespace, so a padded-to-8 column glues the
      // mode onto "Learn" ("UntaggedLearn"), which is not 4 fields, which ends
      // the table — and every untagged membership silently vanishes from the
      // shadow copy. The real CLI prints `Untagged Learn`.
      return `  ${pad(name, 16)} ${pad(mode, 8)} ${pad("Learn", 12)} ${p.link}`;
    })
    .join("\n");
  return ` Status and Counters - VLAN Information - Ports - VLAN ${id}

  VLAN ID : ${id}
  Name : ${v.name}
  Status : Port-based
  Jumbo : No

  Port Information Mode     Unknown VLAN Status
  ---------------- -------- ------------ ----------
${rows}
`;
}

function showSpanningTree(m) {
  const rows = Object.entries(m.ports)
    .map(([name, p]) => {
      const state = p.shut ? "Disabled" : "Forwarding";
      const head = `  ${pad(name, 6)} ${pad(p.media, 8)} | ${pad("20000", 8)} ${pad("128", 4)} ${pad(state, 10)} |`;
      return p.shut ? head : `${head} 643150-bf8c40 2     Yes Yes`;
    })
    .join("\n");
  return `                  |           Prio             | Designated    Hello
  Port  Type      | Cost      rity  State      | Bridge        Time  PtP Edge
  ----- --------- + --------- ----- ---------- + ------------- ----- --- ----
${rows}
`;
}

function showTrunks(m) {
  const rows = Object.entries(m.ports)
    .filter(([_, p]) => p.trunk)
    .map(([name, p]) => `  ${pad(name, 3)} | ${pad(p.name, 32)} ${pad(p.media, 8)} | ${pad(p.trunk, 4)} LACP`)
    .join("\n");
  return ` Load Balancing

  Port | Name                             Type      | Group Type
  ---- + -------------------------------- --------- + ----- -----
${rows}
`;
}

function showInterfaces(m) {
  const rows = Object.entries(m.ports)
    .map(([name, p]) => {
      const label = p.trunk ? `${name}-${p.trunk}` : name;
      return `  ${pad(label, 6)} ${pad("2,993,652...", 11)} ${pad("1,219,890...", 11)} ${pad("0", 11)} ${pad("0", 11)} ${pad("off", 4)} 0`;
    })
    .join("\n");
  return ` Status and Counters - Port Counters

                                                              Flow  Bcast
  Port    Total Bytes  Total Frames Errors Rx    Drops Rx     Ctrl  Limit
  ------- ------------ ------------ ------------ ------------ ----- ------
${rows}
`;
}

function showSystem(m) {
  return ` Status and Counters - System Information

  System Name        : ${m.name}
  System Contact     :
  System Location    : Rack1

  Software revision  : Y.11.44          Base MAC Addr      : 643150-bf8c40
  ROM Version        : N.10.02          Serial Number      : CN117DG07S

  Up Time            : 4 days           Memory   - Total   : 23,556,880
`;
}

/// `show running-config` — the only source of port names and of `shutdown`, and
/// the only read whose output carries secrets, so it goes through
/// `Switch::secret_command`.  No secrets here, and no fake ones either: the
/// shape of the lines is what the parser needs.
function showRunningConfig(m) {
  const blocks = Object.entries(m.ports)
    .filter(([_, p]) => p.name !== "" || p.shut)
    .map(([name, p]) => {
      const lines = [`interface ${name} `];
      if (p.name !== "") lines.push(`   name "${p.name}" `);
      if (p.shut) lines.push("   shutdown ");
      lines.push("exit");
      return lines.join("\n");
    })
    .join("\n");
  const vlans = m.vlans
    .map((v) => {
      const untagged = Object.entries(m.ports)
        .filter(([_, p]) => p.untagged === v.id)
        .map(([n]) => n)
        .join(",");
      const tagged = Object.entries(m.ports)
        .filter(([_, p]) => p.tagged.includes(v.id))
        .map(([n]) => n)
        .join(",");
      const lines = [`vlan ${v.id} `, `   name "${v.name}" `];
      if (untagged) lines.push(`   untagged ${untagged} `);
      if (tagged) lines.push(`   tagged ${tagged} `);
      lines.push("exit");
      return lines.join("\n");
    })
    .join("\n");
  return `${blocks}
trunk 47-48 Trk1 LACP 
${vlans}
ip default-gateway 10.3.1.1 
`;
}

/// A port token as the CLI spells it: `23`, `Trk1`.
const PORT_RE = /^(Trk\d+|\d+(-\d+)?)$/;

/// Expand `2,47-48,Trk1` to the port names it covers.
function expandPorts(spec) {
  const out = [];
  for (const part of spec.split(",")) {
    const p = part.trim();
    if (!PORT_RE.test(p)) throw new Error(`bad port list item: ${p}`);
    const range = p.split("-").map(Number);
    if (range.length === 2 && range.every(Number.isInteger)) {
      for (let n = range[0]; n <= range[1]; n += 1) out.push(String(n));
    } else {
      out.push(p);
    }
  }
  return out;
}

/**
 * One console session.
 *
 * `opts`: `keyGate` (the 2824's "press any key"), `radiusNoise` (the note),
 * `fail` (commands the device refuses, to exercise the failure path),
 * `page` (emit `-- MORE --` once in the middle of a long response).
 */
export function startFakeSwitch(opts = {}) {
  const {
    port = 22323,
    name = "switch1",
    keyGate = false,
    radiusNoise = true,
    model = defaultModel(name),
  } = opts;
  const state = {
    model,
    log: [],
    fail: new Set(),
    sessions: 0,
    sockets: new Set(),
    offline: false,
  };

  const prompt = (phase) => {
    if (phase === "config") return `${model.name}(config)# `;
    if (phase === "exec") return `${model.name}# `;
    return `${model.name}> `;
  };

  /** Answer one command line in the given mode. Returns the text to write. */
  function run(session, line) {
    const cmd = line.trim();
    state.log.push({ mode: session.phase, cmd });

    if (session.phase === "exec" || session.phase === "config") {
      if (state.fail.has(cmd)) {
        // The firmware's refusal, verbatim: `check_cli_error` looks for these.
        return `Invalid input: ${cmd.split(" ").slice(1).join(" ")}\n${prompt(session.phase)}`;
      }
      if (session.phase === "config" && /^(no )?vlan \d+ (tagged|untagged) /.test(cmd)) {
        return applyMembership(session, cmd);
      }
      if (session.phase === "config" && /^interface \S+ (name|enable|disable)/.test(cmd)) {
        return applyInterface(cmd);
      }
    }

    switch (cmd) {
      case "":
        return prompt(session.phase);
      case "show system":
        return `${showSystem(model)}${prompt(session.phase)}`;
      case "show vlan":
        return `${showVlan(model)}${prompt(session.phase)}`;
      case "show spanning-tree":
        return `${showSpanningTree(model)}${prompt(session.phase)}`;
      case "show trunks":
        return `${showTrunks(model)}${prompt(session.phase)}`;
      case "show interfaces":
        return `${showInterfaces(model)}${prompt(session.phase)}`;
      case "show running-config":
        return `${showRunningConfig(model)}${prompt(session.phase)}`;
      case "config":
      case "configure":
        if (session.phase !== "exec") return `Invalid input: ${cmd}\n${prompt(session.phase)}`;
        session.phase = "config";
        // The 2510G paints the echoed command and the new prompt on one row.
        return `${model.name}(config)# `;
      case "exit":
        if (session.phase === "config") {
          session.phase = "exec";
          return prompt("exec");
        }
        session.socket.end();
        return "";
      case "write":
        // The firmware's NVRAM command. The app must never send it; the suite
        // asserts it never appears in the log.
        state.wroteToNvram = true;
        return `Writing current configuration to flash memory.\n${prompt(session.phase)}`;
      default: {
        const m = /^show vlan (\d+)$/.exec(cmd);
        if (m) {
          const out = showVlanPorts(model, Number(m[1]));
          return out === null
            ? `Invalid input: ${cmd}\n${prompt(session.phase)}`
            : `${out}${prompt(session.phase)}`;
        }
        return `Invalid input: ${cmd}\n${prompt(session.phase)}`;
      }
    }
  }

  /** `vlan 225 tagged 7` / `no vlan 225 untagged 23` against the model. */
  function applyMembership(session, cmd) {
    const m = /^(no )?vlan (\d+) (tagged|untagged) (\S+)$/.exec(cmd.trim());
    const remove = Boolean(m[1]);
    const id = Number(m[2]);
    const mode = m[3];
    let ports;
    try {
      ports = expandPorts(m[4]);
    } catch (e) {
      return `Invalid input: ${cmd.trim()}\n${prompt(session.phase)}`;
    }
    if (!model.vlans.some((v) => v.id === id)) {
      // The real CLI's complaint about a VLAN that does not exist.
      return `Error: VLAN ${id} does not exist\n${prompt(session.phase)}`;
    }
    for (const p of ports) {
      const port = model.ports[p];
      if (!port) return `Error: port ${p} does not exist\n${prompt(session.phase)}`;
      if (mode === "untagged") {
        if (remove) port.untagged = null;
        else {
          port.untagged = id;
          port.tagged = port.tagged.filter((v) => v !== id);
        }
      } else if (remove) {
        port.tagged = port.tagged.filter((v) => v !== id);
      } else if (!port.tagged.includes(id)) {
        port.tagged.push(id);
      }
    }
    // Same-row echo and prompt, like every other config command.
    return "";
  }

  function applyInterface(cmd) {
    const m = /^interface (\S+) (name|enable|disable)(?: (.*))?$/.exec(cmd.trim());
    const port = model.ports[m[1]];
    if (!port) return `Error: port ${m[1]} does not exist\n${prompt("config")}`;
    if (m[2] === "name") port.name = (m[3] || "").replace(/^"|"$/g, "");
    if (m[2] === "disable") port.shut = true;
    if (m[2] === "enable") port.shut = false;
    return "";
  }

  const server = net.createServer((socket) => {
    const session = { phase: keyGate ? "gate" : "login", buffer: "", socket };
    state.sessions += 1;
    state.sockets.add(socket);
    socket.on("close", () => state.sockets.delete(socket));
    const say = (s) => socket.write(s);

    say(`\nProCurve J9280A Switch ${model.name}\n`);
    say(`Copyright (C) 1991-2024 Hewlett Packard Enterprise\n`);
    say(keyGate ? `${KEY_GATE}` : `\n${LOGIN}`);

    socket.on("data", (buf) => {
      session.buffer += buf.toString("utf8");
      // The key gate is answered with a bare byte, not a line.
      while (session.phase === "gate" && session.buffer.length > 0) {
        session.buffer = session.buffer.slice(1);
        say(`\n${LOGIN}`);
      }
      let nl;
      while ((nl = session.buffer.indexOf("\n")) >= 0) {
        const line = session.buffer.slice(0, nl).replace(/\r$/, "");
        session.buffer = session.buffer.slice(nl + 1);
        if (session.phase === "login") {
          session.login = line;
          say(`\n${PASSWORD}`);
          session.phase = "password";
          continue;
        }
        if (session.phase === "password") {
          session.phase = "exec";
          // Manager Exec: `enable` on this firmware is a second login, and the
          // same credentials are accepted.
          say(`\n${model.name}> `);
          continue;
        }
        if (line.trim() === "enable") {
          say(`\n${line}\n${LOGIN}`);
          session.phase = "enable-login";
          continue;
        }
        if (session.phase === "enable-login") {
          say(`\n${PASSWORD}`);
          session.phase = "enable-password";
          continue;
        }
        if (session.phase === "enable-password") {
          session.phase = "exec";
          say(radiusNoise ? `\n${RADIUS_NOISE}\n${model.name}# ` : `\n${model.name}# `);
          continue;
        }
        // Echo the command, then its response. `Switch::command` treats a
        // missing echo as a console that never took the line and resends it.
        const out = run(session, line);
        if (session.phase === "config" && out === "") {
          // A silent config command: the echo and the prompt share a row.
          say(`${line}${prompt(session.phase)}`);
        } else {
          say(`\n${line}\n${out}`);
        }
      }
    });
    socket.on("error", () => {});
  });
  server.listen(port, "127.0.0.1");

  const api = http.createServer((req, res) => {
    const send = (code, body) => {
      res.writeHead(code, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    if (req.method === "GET" && req.url === "/state") return send(200, state.model);
    if (req.method === "GET" && req.url === "/log") return send(200, state.log);
    if (req.method === "GET" && req.url === "/meta")
      return send(200, { sessions: state.sessions, wroteToNvram: Boolean(state.wroteToNvram) });
    if (req.method === "POST" && req.url === "/reset") {
      state.log.length = 0;
      state.fail.clear();
      state.wroteToNvram = false;
      // The model goes back to the fixture **in place**: the console sessions
      // close over this object, so replacing it would leave a live session
      // writing to a model nobody reads.  Every test therefore starts from the
      // same device state, which is what makes them order-independent.
      const fresh = defaultModel(model.name);
      model.vlans = fresh.vlans;
      model.ports = fresh.ports;
      return send(200, { ok: true });
    }
    if (req.method === "POST" && req.url === "/fail") {
      let body = "";
      req.on("data", (d) => (body += d));
      req.on("end", () => {
        for (const c of JSON.parse(body || "[]")) state.fail.add(c);
        send(200, { fail: [...state.fail] });
      });
      return undefined;
    }
    if (req.method === "POST" && req.url === "/model") {
      let body = "";
      req.on("data", (d) => (body += d));
      req.on("end", () => {
        Object.assign(state.model, JSON.parse(body));
        send(200, state.model);
      });
      return undefined;
    }
    if (req.method === "POST" && req.url === "/offline") {
      // The device disappears: no more console sessions, and the ones already
      // open die.  This is the state a queued change has to survive — the pass
      // that cannot connect must fail the queue's rows with a reason, not leave
      // them `applying` where nothing can see them for fifteen minutes.
      state.offline = true;
      server.close();
      for (const s of state.sockets) s.destroy();
      state.sockets.clear();
      return send(200, { offline: true });
    }
    if (req.method === "POST" && req.url === "/online") {
      // Only re-listen if we were the one that stopped: `listen` on a listening
      // server is an EADDRINUSE, and an uncaught one takes the fake — and every
      // test after it — down with it.
      if (state.offline) {
        state.offline = false;
        server.listen(port, "127.0.0.1");
      }
      return send(200, { offline: state.offline });
    }
    send(404, { error: "no such endpoint" });
  });

  return {
    model: state.model,
    state,
    server,
    api,
    listen(statePort) {
      api.listen(statePort, "127.0.0.1");
    },
    close() {
      server.close();
      api.close();
    },
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const arg = (name, dflt) => {
    const i = process.argv.indexOf(`--${name}`);
    return i >= 0 ? process.argv[i + 1] : dflt;
  };
  const sw = startFakeSwitch({
    port: Number(arg("telnet", 22323)),
    name: arg("name", "switch1"),
    keyGate: process.argv.includes("--gate"),
  });
  sw.listen(Number(arg("state", 22324)));
  console.log(`fake switch: telnet ${arg("telnet", 22323)}, state ${arg("state", 22324)}`);
}
