const $ = (id) => document.getElementById(id);
const consoleEl = $("console");

function log(text, cls = "") {
  const line = document.createElement("div");
  line.className = `line ${cls}`;
  line.textContent = text;
  consoleEl.prepend(line);
}

const API_KEY = "dev-operator-key";
let pitchLock = false;

function setBusy(on, label) {
  if (pitchLock && !on) {
    return;
  }
  document.querySelectorAll("button").forEach((b) => {
    b.disabled = on;
    b.classList.toggle("busy", on);
  });
  $("busy").classList.toggle("on", on);
  if (on && label) {
    $("script").textContent = label;
  }
}

function showToast(message) {
  const el = $("toast");
  el.hidden = false;
  el.textContent = message;
}

function hideToast() {
  $("toast").hidden = true;
}

async function api(path, opts = {}) {
  const headers = {
    "Content-Type": "application/json",
    "X-Api-Key": API_KEY,
    ...(opts.headers || {}),
  };
  const res = await fetch(path, { ...opts, headers });
  let data = {};
  try {
    data = await res.json();
  } catch {
    data = { detail: "invalid JSON from server" };
  }
  if (!res.ok) {
    const msg = data.detail || `request failed (${res.status})`;
    showToast(msg);
    throw new Error(msg);
  }
  hideToast();
  return data;
}

function stroops(n) {
  return `${n.toLocaleString()} stroops`;
}

function xlm(n) {
  return `${(n / 10_000_000).toFixed(7)} XLM`;
}

let selectedId = "";

function relTime(ts) {
  if (!ts) return "";
  const sec = Math.max(0, (Date.now() / 1000 - Number(ts)) | 0);
  if (sec < 5) return "just now";
  if (sec < 60) return `${sec}s ago`;
  return `${Math.floor(sec / 60)}m ago`;
}

function render(state) {
  $("ledger").textContent = `ledger ${state.ledger.toLocaleString()}`;
  $("remain").textContent = stroops(state.remaining_stroops);
  $("rules").textContent = state.rule_count;

  $("policy").innerHTML = (state.rules || [])
    .map(
      (r) => `<article class="card ${r.resource_id === selectedId ? "selected" : ""}">
        <strong>${r.name}</strong>
        <span class="tag">${r.resource_id}</span>
        <small>${r.method} · cap ${stroops(r.max_spend_per_period)} (${xlm(r.max_spend_per_period)}) · max ${r.max_calls_per_period} calls</small>
      </article>`
    )
    .join("") || `<p class="empty">No policy rules installed.</p>`;

  $("market").innerHTML = (state.resources || [])
    .map(
      (r) => `<article class="card ${r.id === selectedId ? "selected" : ""}">
        <strong>${r.name}</strong>
        <span class="tag">${r.id}</span>
        <small>${r.price} stroops / call · ${xlm(r.price)}</small>
      </article>`
    )
    .join("") || `<p class="empty">Marketplace is empty.</p>`;

  $("bars").innerHTML = (state.rules || [])
    .map(
      (r) => `<div>
        <small>${r.name} · ${r.spent}/${r.max_spend_per_period} used</small>
        <div class="bar"><i style="width:${Math.min(100, r.used_pct)}%"></i></div>
      </div>`
    )
    .join("") || `<p class="empty">No spend yet this period.</p>`;

  $("audit").innerHTML = (state.audit || [])
    .map((e) => {
      const cls = e.decision === "denied" ? "denied" : "approved";
      const extra = e.tx_hash
        ? ` <a class="tx-link" href="https://stellar.expert/explorer/testnet/tx/${e.tx_hash}" target="_blank" rel="noopener noreferrer">tx ${e.tx_hash.slice(0, 8)}…${e.tx_hash.slice(-6)} ↗</a>`
        : "";
      return `<div class="row ${cls}"><span>${e.decision} · ${e.reason} · ${e.resource_id || "policy"} · ${e.amount} · rem ${e.remaining}${extra}</span><time>${relTime(e.ts)}</time></div>`;
    })
    .join("") || `<p class="empty">No auth_decision events yet.</p>`;
}

async function refresh() {
  render(await api("/api/state"));
}

async function discover() {
  setBusy(true, "calling discover_resources…");
  try {
    const query = $("query").value;
    log(`> discover_resources("${query}")`, "dim");
    const data = await api("/api/discover", {
      method: "POST",
      body: JSON.stringify({ query }),
    });
    log(JSON.stringify(data.results, null, 2), "ok");
  } finally {
    setBusy(false);
  }
}

async function budget() {
  setBusy(true, "calling check_budget…");
  try {
    log("> check_budget()", "dim");
    const data = await api("/api/budget");
    log(JSON.stringify(data, null, 2), "ok");
    await refresh();
  } finally {
    setBusy(false);
  }
}

async function pay(id) {
  setBusy(true, `calling pay_and_call ${id}…`);
  try {
    log(`> pay_and_call("${id}", "{}")`, "dim");
    const data = await api("/api/pay", {
      method: "POST",
      body: JSON.stringify({ resource_id: id, params: "{}" }),
    });
    log(JSON.stringify(data, null, 2), data.ok ? "ok" : "err");
    if (!data.ok) {
      showToast(`${data.error}: ${data.reason || "policy denied"}`);
    } else {
      selectedId = id;
      if (data.tx_hash) {
        log(`Explorer: https://stellar.expert/explorer/testnet/tx/${data.tx_hash}`, "ok");
      }
    }
    await refresh();
    return data;
  } finally {
    setBusy(false);
  }
}

$("discover").onclick = discover;
$("budget-btn").onclick = budget;
$("reset").onclick = async () => {
  if (!window.confirm("Reset the live pitch demo? Current spend and audit events will be wiped.")) {
    return;
  }
  await api("/api/reset", { method: "POST" });
  consoleEl.innerHTML = "";
  $("script").innerHTML = "Demo reset. Policy regenerated from 75 synthetic transactions.";
  await refresh();
};
document.querySelectorAll("[data-pay]").forEach((btn) => {
  btn.onclick = () => pay(btn.dataset.pay);
});

const PITCH = [
  {
    say: "1/6 Simulate observed agent traffic, then generate a least-privilege policy.",
    run: refresh,
  },
  {
    say: "2/6 The agent discovers a paid weather oracle over MCP.",
    run: async () => {
      $("query").value = "weather";
      await discover();
    },
  },
  {
    say: "3/6 It checks remaining budget on the smart account before spending.",
    run: budget,
  },
  {
    say: "4/6 pay_and_call succeeds under the weather cap via Soroban / x402. Spend is recorded on-chain.",
    run: () => pay("weather-oracle"),
  },
  {
    say: "5/6 A second weather call exceeds the p95-derived cap and is denied on-chain.",
    run: () => pay("weather-oracle"),
  },
  {
    say: "6/6 Price feed is still allowed — policies are scoped per vendor payee (asset, pay_to), not a global wallet.",
    run: () => pay("price-feed"),
  },
];

$("pitch").onclick = async () => {
  pitchLock = true;
  setBusy(true, "Running pitch demo…");
  try {
    await api("/api/reset", { method: "POST" });
    consoleEl.innerHTML = "";
    for (const step of PITCH) {
      $("script").innerHTML = step.say;
      await step.run();
      await new Promise((r) => setTimeout(r, 1600));
    }
    $("script").innerHTML =
      "Done. Every approve/deny is an <strong>auth_decision</strong> event. Reset and run again.";
  } finally {
    pitchLock = false;
    setBusy(false);
  }
};

refresh();
