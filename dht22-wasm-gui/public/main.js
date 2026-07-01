// wasm-bindgen `--target web` emits an ES module + a default `init()` that fetches
// and instantiates the .wasm. Everything below runs only after init() resolves.
//
// The wasm side (Rust) is just the iroh + dht22-proto data path: it streams each
// sensor `Record` here as a plain object. All presentation — the text log and the
// uPlot chart — lives in this file. `uPlot` is a global from the classic script tag
// in index.html (loaded before this module).
import init, { Node } from "./wasm/dht22_wasm_gui.js";

const $status = document.querySelector("#status");
const $id = document.querySelector("#endpoint-id");
const $ticket = document.querySelector("#ticket");
const $connect = document.querySelector("#connect");
const $feedStatus = document.querySelector("#feed-status");
const $chart = document.querySelector("#chart");
const $log = document.querySelector("#log");
const $reset = document.querySelector("#reset-identity");

await init();

// Persist the endpoint's secret key so the identity (endpoint ID) is stable
// across reloads. NOTE: this is a private key sitting in localStorage — fine for
// a demo, not for anything sensitive. Clear it with `localStorage.clear()` (or the
// reset below) to roll a fresh identity.
const SECRET_STORAGE_KEY = "iroh-gui:secret";

let node;
try {
  $status.textContent = "spawning iroh endpoint…";
  // null when absent → wasm sees `None` → generates a fresh key.
  node = await Node.spawn(localStorage.getItem(SECRET_STORAGE_KEY));
  // Persist (idempotent if it already existed; stores the new one otherwise).
  localStorage.setItem(SECRET_STORAGE_KEY, node.secret_hex());

  const id = node.endpoint_id();
  $id.textContent = id;
  // Click the ID to select it for copying.
  $id.addEventListener("click", () => getSelection().selectAllChildren($id));

  $connect.disabled = false;
  $status.textContent = "endpoint ready ✓";
} catch (err) {
  $id.textContent = "—";
  $status.textContent = `failed to spawn endpoint: ${err}`;
  console.error(err);
}

// --- text log: same line layout as the native CLI's println! output ---

function fmtReading(r) {
  return r ? `${r.temperature.toFixed(1)}°C ${r.humidity.toFixed(1)}%` : "--";
}

function fmtRecord(rec) {
  return (
    `[id ${String(rec.id).padStart(6)} t=${String(rec.time).padStart(10)}] ` +
    `inside: ${fmtReading(rec.inside)}  outside: ${fmtReading(rec.outside)}  ` +
    `fan: ${rec.fan ? "on" : "off"}`
  );
}

function appendLog(line) {
  $log.textContent += line + "\n";
  $log.scrollTop = $log.scrollHeight; // keep the newest line in view
}

// --- chart: temperature (left axis) + humidity (right axis) vs. time ---

const WINDOW_SECONDS = 24 * 3600; // only plot the last 24h (relative to the newest point)
const MAX_POINTS = 50000; // hard safety cap regardless of the time window

// uPlot data is column-oriented: [xs, inside°C, outside°C, inside%RH, outside%RH].
let data = [[], [], [], [], []];
let chart = null;
let redrawQueued = false;

// Per-series forward-fill state: the last real value (repeated across gaps to keep
// the line continuous) and a parallel "was this point a genuine reading?" mask,
// which drives the × markers — so held/repeated points are visually distinct.
let lastVals = [null, null, null, null, null];
let realMask = [null, [], [], [], []];

// Per-point fan on/off (the Record's `fan` field), shaded as background bands.
let fanMask = [];
const FAN_FILL = "rgba(40, 180, 99, 0.22)"; // translucent green = fan on
const FAN_MIN_PX = 3; // minimum band width (CSS px) so a 1-sample blip stays visible

// Series 1-4 → value scale and marker color (matches the series strokes below).
const SERIES_SCALE = { 1: "C", 2: "C", 3: "%", 4: "%" };
const SERIES_COLOR = { 1: "#e4572e", 2: "#f3a712", 3: "#4e6bff", 4: "#17bebb" };

// Mid-gray axis styling reads fine on both light and dark color schemes.
const AXIS = {
  stroke: "#888",
  grid: { stroke: "rgba(128,128,128,0.18)" },
  ticks: { stroke: "rgba(128,128,128,0.3)" },
};

function makeChart() {
  const opts = {
    width: $chart.clientWidth || 600,
    height: 280,
    scales: { x: { time: true }, C: {}, "%": { range: [0, 100] } },
    series: [
      {},
      { label: "inside °C", scale: "C", stroke: "#e4572e", width: 2 },
      { label: "outside °C", scale: "C", stroke: "#f3a712", width: 2 },
      { label: "inside %RH", scale: "%", stroke: "#4e6bff", width: 1, dash: [4, 3] },
      { label: "outside %RH", scale: "%", stroke: "#17bebb", width: 1, dash: [4, 3] },
    ],
    axes: [
      { ...AXIS },
      { ...AXIS, scale: "C", label: "°C" },
      { ...AXIS, scale: "%", label: "%RH", side: 1, grid: { show: false } },
    ],
    // drawClear (background, before series): shade where the fan is on.
    // draw (foreground): × markers on genuine samples.
    hooks: { drawClear: [drawFanBands], draw: [drawRealMarkers] },
  };
  return new uPlot(opts, data, $chart);
}

// Draw an × at every real sample of each series (clipped to the plot area). Held /
// forward-filled points have no marker, so flat repeated segments read as gaps.
function drawRealMarkers(u) {
  const ctx = u.ctx;
  const pr = u.pxRatio || self.devicePixelRatio || 1;
  const xs = data[0];
  const half = 3 * pr;
  ctx.save();
  ctx.beginPath();
  ctx.rect(u.bbox.left, u.bbox.top, u.bbox.width, u.bbox.height);
  ctx.clip();
  ctx.lineWidth = 1.5 * pr;
  for (let s = 1; s <= 4; s++) {
    const ys = data[s];
    const mask = realMask[s];
    ctx.strokeStyle = SERIES_COLOR[s];
    let lastPx = -Infinity;
    for (let i = 0; i < mask.length; i++) {
      if (!mask[i] || ys[i] == null) continue;
      const px = u.valToPos(xs[i], "x", true);
      if (px - lastPx < 5 * pr) continue; // too dense to distinguish; thin out
      lastPx = px;
      const py = u.valToPos(ys[i], SERIES_SCALE[s], true);
      ctx.beginPath();
      ctx.moveTo(px - half, py - half);
      ctx.lineTo(px + half, py + half);
      ctx.moveTo(px - half, py + half);
      ctx.lineTo(px + half, py - half);
      ctx.stroke();
    }
  }
  ctx.restore();
}

// Shade the background green over every contiguous span where the fan is on. Runs
// at drawClear so it sits behind the series lines. Band edges fall at the midpoints
// between samples so adjacent on-points form one seamless band.
function drawFanBands(u) {
  const xs = data[0];
  const n = xs.length;
  if (!n) return;
  const ctx = u.ctx;
  const pr = u.pxRatio || self.devicePixelRatio || 1;
  ctx.save();
  ctx.beginPath();
  ctx.rect(u.bbox.left, u.bbox.top, u.bbox.width, u.bbox.height);
  ctx.clip();
  ctx.fillStyle = FAN_FILL;
  let i = 0;
  while (i < n) {
    if (!fanMask[i]) { i++; continue; }
    const start = i;
    while (i < n && fanMask[i]) i++;
    const end = i - 1; // inclusive run [start, end]
    const leftVal = start > 0 ? (xs[start - 1] + xs[start]) / 2 : xs[start];
    const rightVal = end < n - 1 ? (xs[end] + xs[end + 1]) / 2 : xs[end];
    const lpx = u.valToPos(leftVal, "x", true);
    const rpx = u.valToPos(rightVal, "x", true);
    ctx.fillRect(lpx, u.bbox.top, Math.max(rpx - lpx, FAN_MIN_PX * pr), u.bbox.height);
  }
  ctx.restore();
}

// Coalesce bursts (e.g. the history backfill streams many records at once) into one
// redraw per animation frame, so we don't re-render the chart per record.
function scheduleRedraw() {
  if (redrawQueued) return;
  redrawQueued = true;
  requestAnimationFrame(() => {
    redrawQueued = false;
    if (chart) chart.setData(data);
  });
}

function resetChart() {
  data = [[], [], [], [], []];
  realMask = [null, [], [], [], []];
  lastVals = [null, null, null, null, null];
  fanMask = [];
  if (chart) chart.setData(data);
}

// Push one series value with forward-fill: repeat the last real value across a gap,
// and record whether this point was a genuine reading (drives the × markers).
function pushSeries(sidx, reading, field) {
  if (reading) {
    const v = reading[field];
    lastVals[sidx] = v;
    data[sidx].push(v);
    realMask[sidx].push(true);
  } else {
    data[sidx].push(lastVals[sidx]); // null until the first real reading arrives
    realMask[sidx].push(false);
  }
}

function pushPoint(rec) {
  const xs = data[0];
  const x = rec.time;
  // uPlot needs strictly-increasing x. Skip points with an unusable timestamp
  // (device clock not set yet → time 0) or one that doesn't advance; such records
  // still show up in the text log below.
  if (x <= 0 || (xs.length && x <= xs[xs.length - 1])) return;
  data[0].push(x);
  pushSeries(1, rec.inside, "temperature");
  pushSeries(2, rec.outside, "temperature");
  pushSeries(3, rec.inside, "humidity");
  pushSeries(4, rec.outside, "humidity");
  fanMask.push(!!rec.fan);

  // Trim the front to the last 24h (this also drops any pre-clock-sync points that
  // landed near the 1970 epoch, once a real timestamp arrives), then the hard cap.
  const cutoff = x - WINDOW_SECONDS;
  let drop = 0;
  while (drop < data[0].length && data[0][drop] < cutoff) drop++;
  drop = Math.max(drop, data[0].length - MAX_POINTS);
  if (drop > 0) {
    for (const col of data) col.splice(0, drop);
    for (let s = 1; s <= 4; s++) realMask[s].splice(0, drop);
    fanMask.splice(0, drop);
  }
  scheduleRedraw();
}

window.addEventListener("resize", () => {
  if (chart) chart.setSize({ width: $chart.clientWidth, height: 280 });
});

// --- record stream from wasm ---

let lastId = null;

// Called by the Rust loop for every record (history backfill + live tail). `rec` is
// the serialized dht22_proto::Record: { id, time, inside, outside, fan }.
function onRecord(rec) {
  // The device restarting resets ids; the Rust side re-seeds, and we clear the
  // chart so the old and new series don't get joined across the gap.
  if (lastId !== null && rec.id < lastId) {
    appendLog("(device restarted — clearing chart)");
    resetChart();
  }
  lastId = rec.id;
  appendLog(fmtRecord(rec));
  pushPoint(rec);
}

function setFeedStatus(text) {
  $feedStatus.textContent = text;
}

// Start the live feed. The Rust side backfills history then tails live, calling
// onRecord per record. No unsubscribe yet, so this is one-shot: the button disables.
$connect.addEventListener("click", () => {
  const ticket = $ticket.value.trim();
  if (!ticket) {
    setFeedStatus("enter a sensor server ticket first");
    return;
  }

  $connect.disabled = true;
  $ticket.disabled = true;
  $log.textContent = "";
  lastId = null;
  resetChart();
  if (!chart) chart = makeChart();
  document.querySelector("#chart-hint").hidden = false;
  setFeedStatus("connecting…");
  node.subscribe(ticket, onRecord, setFeedStatus);
});

// Roll a fresh identity: drop the stored secret and reload so spawn generates a
// new key.
$reset.addEventListener("click", () => {
  localStorage.removeItem(SECRET_STORAGE_KEY);
  location.reload();
});
