// Hand-rolled, dependency-free canvas charts (no chart library — minimal deps).

interface CanvasCtx {
  ctx: CanvasRenderingContext2D;
  w: number;
  h: number;
}

interface PlotSeries {
  data: (number | null)[];
  color: string;
  fill?: boolean;
  window?: number;
  width?: number;
}

const pad2 = (n: number) => (n < 10 ? "0" + n : "" + n);
function fmtTimeLocal(ts: number): string {
  const d = new Date(ts * 1000);
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
}

function setupCanvas(canvas: HTMLCanvasElement, cssHeight: number): CanvasCtx {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth || 300;
  canvas.width = Math.max(1, Math.round(w * dpr));
  canvas.height = Math.max(1, Math.round(cssHeight * dpr));
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    const c = document.createElement("canvas");
    c.width = c.height = 1;
    return { ctx: c.getContext("2d")!, w, h: cssHeight };
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h: cssHeight };
}

function niceMax(v: number): number {
  if (v <= 0) return 1;
  const exp = Math.pow(10, Math.floor(Math.log10(v)));
  const m = v / exp;
  if (m <= 1) return exp;
  if (m <= 2) return 2 * exp;
  if (m <= 5) return 5 * exp;
  return 10 * exp;
}

export function drawChart(
  canvas: HTMLCanvasElement,
  series: PlotSeries[],
  opts: { fmt: (v: number) => string; height: number; ts?: number[] },
): void {
  const { ctx, w, h } = setupCanvas(canvas, opts.height);
  const padL = 58, padR = 8, padT = 8, padB = 20;
  const pw = w - padL - padR, ph = h - padT - padB;

  let maxv = 0;
  for (const s of series) {
    const start = Math.max(0, s.data.length - (s.window ?? s.data.length));
    for (let i = start; i < s.data.length; i++) {
      const v = s.data[i];
      if (v != null && v > maxv) maxv = v;
    }
  }
  const ymax = niceMax(maxv * 1.05);
  const n = series.length ? series[0].data.length : 0;

  ctx.clearRect(0, 0, w, h);
  ctx.font = "11px ui-monospace, monospace";

  ctx.strokeStyle = "#232d38";
  ctx.fillStyle = "#8b98a5";
  ctx.lineWidth = 1;
  for (let g = 0; g <= 4; g++) {
    const y = padT + ph - (ph * g) / 4;
    ctx.beginPath();
    ctx.moveTo(padL, y);
    ctx.lineTo(w - padR, y);
    ctx.stroke();
    ctx.fillText(opts.fmt((ymax * g) / 4), 4, y + 4);
  }

  // x labels (3). A series can be shorter than ts (an interface that
  // appeared later in the history), so offset into the timestamp array.
  const ts = opts.ts;
  if (n > 1 && ts) {
    const off = ts.length - n;
    const pts = [0, Math.floor((n - 1) / 2), n - 1];
    for (const i of pts) {
      const x = padL + (pw * i) / (n - 1);
      const right = i === n - 1, mid = i === Math.floor((n - 1) / 2);
      ctx.textAlign = right ? "right" : mid ? "center" : "left";
      ctx.fillText(fmtTimeLocal(ts[off + i] || 0), x, h - 5);
    }
    ctx.textAlign = "left";
  }

  if (n === 0) {
    ctx.fillStyle = "#8b98a5";
    ctx.fillText("no data yet", padL + 8, padT + ph / 2);
    return;
  }

  for (const s of series) {
    const win_s = s.window ?? s.data.length;
    const start = Math.max(0, s.data.length - win_s);
    const count = s.data.length - start;
    if (count < 2) continue;
    const xAt = (i: number) => padL + (pw * (i - start)) / (count - 1);
    const yAt = (v: number) => padT + ph - (ph * Math.min(v, ymax)) / ymax;

    if (s.fill) {
      ctx.beginPath();
      let started = false;
      let firstX: number | null = null, lastX: number | null = null;
      for (let i = start; i < s.data.length; i++) {
        const v = s.data[i];
        if (v == null) { started = false; continue; }
        const x = xAt(i), y = yAt(v);
        if (!started) { ctx.moveTo(x, y); started = true; }
        else ctx.lineTo(x, y);
        if (firstX === null) firstX = x;
        lastX = x;
      }
      if (firstX !== null && lastX !== null) {
        ctx.lineTo(lastX, padT + ph);
        ctx.lineTo(firstX, padT + ph);
        ctx.closePath();
        const grad = ctx.createLinearGradient(0, padT, 0, padT + ph);
        grad.addColorStop(0, s.color + "55");
        grad.addColorStop(1, s.color + "05");
        ctx.fillStyle = grad;
        ctx.fill();
      }
    }

    ctx.beginPath();
    let started = false;
    for (let i = start; i < s.data.length; i++) {
      const v = s.data[i];
      if (v == null) { started = false; continue; }
      const x = xAt(i), y = yAt(v);
      if (!started) { ctx.moveTo(x, y); started = true; }
      else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = s.color;
    ctx.lineWidth = s.width ?? 1.8;
    ctx.lineJoin = "round";
    ctx.stroke();
  }
}

export function sparkline(canvas: HTMLCanvasElement, series: PlotSeries[], window = 60): void {
  const { ctx, w, h } = setupCanvas(canvas, 46);
  ctx.clearRect(0, 0, w, h);
  const n = series.length ? series[0].data.length : 0;
  const start = Math.max(0, n - window);
  const count = n - start;
  if (count < 2) {
    ctx.fillStyle = "#4a5560";
    ctx.font = "10px ui-monospace, monospace";
    ctx.fillText("…", 4, h / 2 + 3);
    return;
  }
  let maxv = 0;
  for (const s of series)
    for (let i = start; i < n; i++) {
      const v = s.data[i];
      if (v != null && v > maxv) maxv = v;
    }
  const ymax = niceMax(maxv * 1.1) || 1;
  const xAt = (i: number) => (w * (i - start)) / (count - 1);
  const yAt = (v: number) => h - 2 - ((h - 6) * Math.min(v, ymax)) / ymax;
  for (const s of series) {
    ctx.beginPath();
    let started = false;
    for (let i = start; i < n; i++) {
      const v = s.data[i];
      if (v == null) { started = false; continue; }
      const x = xAt(i), y = yAt(v);
      if (!started) { ctx.moveTo(x, y); started = true; }
      else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = s.color;
    ctx.lineWidth = 1.5;
    ctx.stroke();
  }
}
