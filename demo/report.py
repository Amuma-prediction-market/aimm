"""Writes one self-contained HTML page comparing AIMM (ℓ = 0.3 L) with an AMM holding the same money (ℓ = L), from the
replays of core/examples/replay.rs. No dependencies.

    python3 demo/report.py nyc london      reads $AIMM_DATA/<key>.csv and .replay, writes $AIMM_DATA/report.html"""
import csv, datetime, html, json, os, sys

DATA = os.environ.get("AIMM_DATA", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "aimm-data"))
W, H, PAD_L, PAD_R, PAD_T, PAD_B = 860, 230, 56, 16, 12, 26


def series(rows, market, win, field):
    return [(int(r["time"]), float(r[field])) for r in rows if r["market"] == market and int(r["range"]) == win]


def path(points, x, y):
    out = []
    for k, (t, v) in enumerate(points):
        if k:
            out.append(f"H{x(t):.1f}")
        out.append(f"{'M' if not k else 'V'}{y(v):.1f}" if k else f"M{x(t):.1f},{y(v):.1f}")
    return "".join(out)


def band(lo, hi, x, y):
    top = "".join(f"{'M' if k == 0 else 'L'}{x(t):.1f},{y(v):.1f}" for k, (t, v) in enumerate(hi))
    bot = "".join(f"L{x(t):.1f},{y(v):.1f}" for t, v in reversed(lo))
    return top + bot + "Z"


def chart(cid, t0, t1, ymax, ylabel, fmt, lines, area=None):
    x = lambda t: PAD_L + (t - t0) / (t1 - t0) * (W - PAD_L - PAD_R)
    y = lambda v: PAD_T + (1 - v / ymax) * (H - PAD_T - PAD_B)
    g = []
    for k in range(5):
        v = ymax * k / 4
        g.append(f'<line class="grid" x1="{PAD_L}" x2="{W - PAD_R}" y1="{y(v):.1f}" y2="{y(v):.1f}"/>'
                 f'<text class="tick" x="{PAD_L - 6}" y="{y(v) + 4:.1f}" text-anchor="end">{fmt(v)}</text>')
    for k in range(6):
        t = t0 + (t1 - t0) * k / 5
        g.append(f'<text class="tick" x="{x(t):.1f}" y="{H - 6}" text-anchor="middle">{datetime.datetime.fromtimestamp(t, datetime.timezone.utc):%H:%M}</text>')
    if area:
        g.append(f'<path class="band" d="{band(area[0], area[1], x, y)}"/>')
    for cls, pts in lines:
        g.append(f'<path class="{cls}" d="{path(pts, x, y)}"/>')
    g.append(f'<line class="cross" id="{cid}-x" y1="{PAD_T}" y2="{H - PAD_B}" x1="-10" x2="-10"/>')
    g.append(f'<text class="axis" x="12" y="{PAD_T + (H - PAD_T - PAD_B) / 2}" transform="rotate(-90 12 {PAD_T + (H - PAD_T - PAD_B) / 2})" text-anchor="middle">{ylabel}</text>')
    return f'<svg id="{cid}" viewBox="0 0 {W} {H}" role="img" aria-label="{html.escape(ylabel)}">{"".join(g)}</svg>'


def summary(rows, market):
    R = [r for r in rows if r["market"] == market]
    wid = [float(r["upper"]) - float(r["lower"]) for r in R]
    err = [abs(float(r["price"]) - float(r["target"])) for r in R]
    return max(float(r["slippage"]) for r in R), sum(wid) / len(wid), max(wid), sum(err) / len(err), min(float(r["payout"]) for r in R)


def settlement(key, k):
    """What the operator got back, in dollars, and what it put in: settle <market> <winner> <payout> <real> <informed> <surplus> <fees> <dust>."""
    lines = [l.split() for l in open(os.path.join(DATA, f"{key}.ops"))]
    f = next(l for l in lines if l[0] == "settle" and l[1] == str(k))
    fund = next(l for l in lines if l[0] == "open" and l[1] == str(k))
    return (int(f[6]) + int(f[7])) / 1e6, int(fund[3]) / 1e6, sum(1 for l in lines if l[0] == str(k) and l[1].endswith("_sell"))


def section(key):
    head = open(os.path.join(DATA, f"{key}.replay"), encoding="utf-8").readline()
    title = head.split(" ranges:")[0].lstrip("# ").strip()
    labels = head.split("ranges: ")[1].split(" winner:")[0].split(" | ")
    win = int(head.rsplit("winner: ", 1)[1])
    rows = list(csv.DictReader(open(os.path.join(DATA, f"{key}.csv"))))
    trades = [l.split() for l in open(os.path.join(DATA, f"{key}.replay")) if l.startswith("X ")]
    dropped = open(os.path.join(DATA, f"{key}.replay"), encoding="utf-8").readlines()[1].lstrip("# ").strip()
    volume = sum(float(t[4]) * float(t[5]) for t in trades)
    tgt, ap, lo, hi = (series(rows, "AIMM", win, f) for f in ("target", "price", "lower", "upper"))
    mp = series(rows, "AMM", win, "price")
    t0, t1 = tgt[0][0], tgt[-1][0]
    p = chart(f"{key}-p", t0, t1, 1.0, "probability", lambda v: f"{v:.0%}",
              [("pm", tgt), ("amm", mp), ("aimm", ap)], area=(lo, hi))
    sa, sm = series(rows, "AIMM", win, "slippage"), series(rows, "AMM", win, "slippage")
    smax = max(v for _, v in sm) * 1.08
    s = chart(f"{key}-s", t0, t1, smax, "cumulative slippage ($)", lambda v: f"${v:,.0f}", [("amm", sm), ("aimm", sa)])
    A, M, F = summary(rows, "AIMM"), summary(rows, "AMM"), summary(rows, "AMM full")
    table = ("<table><tr><th>Market</th><th>Real money</th><th>Slippage on the real trades</th><th>Share of volume</th>"
             "<th>Mean interval width</th><th>Widest interval</th><th>Mean gap to the black line</th><th>Lowest payout</th>"
             "<th>Sales</th><th>Operator's result</th></tr>")
    for k, (name, (sl, mw, xw, er, low)) in enumerate((("AIMM, ℓ = 0.3 L", A), ("AMM, ℓ = L, same money", M), ("AMM, ℓ = L, AIMM's depth", F))):
        back, fund, sales = settlement(key, k)
        table += (f"<tr><td>{name}</td><td>${fund:,.0f}</td><td>${sl:,.0f}</td><td>{sl / volume:.1%}</td><td>{mw * 100:.2f} pts</td>"
                  f"<td>{xw * 100:.1f} pts</td><td>{er * 100:.2f} pts</td><td>{low:.3f}</td><td>{sales:,}</td><td>{back - fund:+,.0f}</td></tr>")
    table += "</table>"
    data = json.dumps(dict(t=[t for t, _ in tgt], pm=[v for _, v in tgt], aimm=[v for _, v in ap], lo=[v for _, v in lo], hi=[v for _, v in hi],
                           amm=[v for _, v in mp], sa=[v for _, v in sa], sm=[v for _, v in sm]))
    return (f'<section><h2>{html.escape(title)}</h2><p class="sub">Winning range {html.escape(labels[win])}. '
            f"{len(trades)} real trades, ${volume:,.0f} at Polymarket's prices, in the last ten hours before it was decided. "
            f"{html.escape(dropped)}.</p>"
            f'<div class="legend"><span><i class="k pm"></i>Polymarket, floored at 1% and normalised</span><span><i class="k aimm"></i>AIMM price, ℓ = 0.3 L</span>'
            f'<span><i class="k band"></i>AIMM information interval</span><span><i class="k amm"></i>AMM price, ℓ = L, same $1,000</span></div>'
            f'<div class="chart" data-key="{key}">{p}<div class="tip" id="{key}-tip"></div></div>{s}{table}'
            f'<script type="application/json" id="{key}-data">{data}</script></section>')


CSS = """
:root{--surface:#fcfcfb;--ink:#0b0b0b;--ink2:#52514e;--grid:#e1e0d9;--s1:#2a78d6;--s2:#eb6834}
@media (prefers-color-scheme:dark){:root{--surface:#1a1a19;--ink:#f0efec;--ink2:#c3c2b7;--grid:#383835;--s1:#3987e5;--s2:#d95926}}
body{background:var(--surface);color:var(--ink);font:15px/1.5 Georgia,serif;margin:0 auto;max-width:900px;padding:24px 16px}
h1{font-size:24px;margin:0 0 6px}h2{font-size:18px;margin:32px 0 2px}.sub,.note{color:var(--ink2);margin:0 0 10px}
svg{width:100%;height:auto;display:block;overflow:visible}.grid{stroke:var(--grid);stroke-width:1}.tick,.axis{fill:var(--ink2);font-size:11px;font-family:system-ui,sans-serif}
path{fill:none;stroke-width:2}.pm{stroke:var(--ink);stroke-width:1.5}.aimm{stroke:var(--s1)}.amm{stroke:var(--s2);stroke-dasharray:5 3}
.band{fill:var(--s1);fill-opacity:.25;stroke:none}.cross{stroke:var(--ink2);stroke-width:1}
.legend{display:flex;flex-wrap:wrap;gap:14px;font:13px system-ui,sans-serif;color:var(--ink2);margin:4px 0}
.k{display:inline-block;width:18px;height:3px;vertical-align:middle;margin-right:6px}.k.pm{background:var(--ink)}.k.aimm{background:var(--s1)}
.k.amm{background:repeating-linear-gradient(90deg,var(--s2) 0 5px,transparent 5px 8px)}.k.band{background:var(--s1);opacity:.35;height:10px}
.chart{position:relative}.tip{position:absolute;pointer-events:none;background:var(--surface);border:1px solid var(--grid);padding:6px 8px;font:12px system-ui,sans-serif;display:none;white-space:nowrap}
table{border-collapse:collapse;width:100%;font:13px system-ui,sans-serif;margin:10px 0}th,td{border-bottom:1px solid var(--grid);padding:5px 6px;text-align:right}th:first-child,td:first-child{text-align:left}
"""
JS = """
document.querySelectorAll('.chart').forEach(c=>{const k=c.dataset.key,d=JSON.parse(document.getElementById(k+'-data').textContent),
svg=c.querySelector('svg'),tip=document.getElementById(k+'-tip'),line=document.getElementById(k+'-p-x'),t0=d.t[0],t1=d.t[d.t.length-1];
const pct=v=>(v*100).toFixed(1)+'%';
svg.addEventListener('mousemove',e=>{const r=svg.getBoundingClientRect(),sx=(e.clientX-r.left)/r.width*860;
const t=t0+(sx-56)/(860-72)*(t1-t0);let i=d.t.findIndex(x=>x>t);i=i<0?d.t.length-1:Math.max(0,i-1);
const x=56+(d.t[i]-t0)/(t1-t0)*(860-72);line.setAttribute('x1',x);line.setAttribute('x2',x);
tip.style.display='block';tip.style.left=Math.min(r.width-190,e.clientX-r.left+12)+'px';tip.style.top='10px';
tip.innerHTML=new Date(d.t[i]*1000).toISOString().slice(11,16)+' UTC<br>Polymarket, floored and normalised '+pct(d.pm[i])+'<br>AIMM '+pct(d.aimm[i])+' ['+pct(d.lo[i])+', '+pct(d.hi[i])+']<br>AMM '+pct(d.amm[i])+'<br>slippage AIMM $'+d.sa[i].toFixed(0)+', AMM $'+d.sm[i].toFixed(0);});
svg.addEventListener('mouseleave',()=>{tip.style.display='none';line.setAttribute('x1',-10);line.setAttribute('x2',-10);});});
"""


def main():
    keys = sys.argv[1:] or ["nyc", "london"]
    body = "".join(section(k) for k in keys)
    page = (f"<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
            f"<title>AIMM against an AMM</title><style>{CSS}</style><h1>AIMM against an AMM with the same money</h1>"
            "<p class='note'>Real Polymarket trades replayed on the contract's own arithmetic. AIMM holds $1,000 against a virtual "
            "liquidity of $3,333; the AMM holds the same $1,000 as its whole liquidity. An informed trader who believes the black "
            "line trades as the whitepaper's Proposition 3 says, so every market follows that line by construction: the gap to it "
            "measures the trader's tolerance, not the market's accuracy. Both accounts sell what they hold before they buy. No fees. "
            "Slippage is what each real trade paid above the market's own price just before it. The operator's result is what it "
            "got back less its fund; AIMM's lower slippage is paid for there, and the payout never fell far enough to test a short pot "
            "at settlement.</p>" + body + f"<script>{JS}</script></html>")
    out = os.path.join(DATA, "report.html")
    open(out, "w", encoding="utf-8").write(page)
    print("wrote", out)


if __name__ == "__main__":
    main()
