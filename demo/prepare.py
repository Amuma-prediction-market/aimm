"""Turns a fetched Polymarket question into the replay's input: the last ten hours before it was decided, minute by
minute. Each line is either a target (Polymarket's prices, floored at 1% and normalised to add up to 1) or a real trade.
Fills priced outside 0.5% to 99.5% are left out, because a curve floored at 1% cannot mirror them; the second line of
the output says how many.

    python3 demo/prepare.py nyc      writes $AIMM_DATA/nyc.replay
Line formats (one event per line, in time order):
    T <unix time> <price_0> ... <price_n-1>             the floored, normalised prices at that minute, adding up to 1
    X <unix time> <range> <yes 1|0> <shares> <price>    a real trade, as a purchase: SELL YES is a purchase of NO
"""
import json, os, sys

DATA = os.environ.get("AIMM_DATA", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "aimm-data"))
WINDOW = 10 * 3600
FLOOR = 0.01           # every price, every minute: the curve's depth is L / ln(1 / lowest opening price)


def price_at(history, t):
    p = history[0][1]
    for s, q in history:
        if s > t:
            break
        p = q
    return p


def main():
    key = sys.argv[1]
    d = json.load(open(os.path.join(DATA, f"{key}.json")))
    rs = d["ranges"]
    win = next(r for r in rs if r["won"])
    # decided: the last minute at which the winning range was still below 95%
    decided = max(t for t, p in win["history"] if p < 0.95)
    start = decided - WINDOW
    lines = []
    dropped, dropped_cash, seen = 0, 0.0, 0
    for t in range(start, decided + 1, 60):
        ps = [max(FLOOR, min(1 - FLOOR, price_at(r["history"], t))) for r in rs]
        s = sum(ps)
        lines.append((t, 0, "T %d %s" % (t, " ".join(f"{p / s:.6f}" for p in ps))))
    for i, r in enumerate(rs):
        for tr in r["trades"]:
            if not start < tr["t"] <= decided:
                continue
            seen += 1
            if not 0.005 < tr["price"] < 0.995:
                dropped += 1
                dropped_cash += tr["size"] * tr["price"]
                continue
            yes = (tr["outcome"] == "Yes") == (tr["side"] == "BUY")
            price = tr["price"] if tr["side"] == "BUY" else 1 - tr["price"]
            lines.append((tr["t"], 1, f"X {tr['t']} {i} {int(yes)} {tr['size']:.6f} {price:.6f}"))
    lines.sort(key=lambda x: (x[0], x[1]))
    out = os.path.join(DATA, f"{key}.replay")
    with open(out, "w") as fh:
        fh.write(f"# {d['title']} ranges: {' | '.join(r['label'] for r in rs)} winner: {rs.index(win)}\n")
        fh.write(f"# dropped {dropped} of {seen} fills priced outside 0.5% to 99.5%, ${dropped_cash:,.0f} at their own price\n")
        fh.write("\n".join(l for _, _, l in lines) + "\n")
    n_trades = sum(1 for _, k, _ in lines if k == 1)
    print(f"{key}: window {start}..{decided}, {len(lines) - n_trades} minutes, {n_trades} real trades -> {out}")


if __name__ == "__main__":
    main()
