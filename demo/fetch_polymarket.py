"""Fetches the trading history replayed by the demo: two resolved Polymarket weather questions, every temperature
range of each, with each range's minute-by-minute price and every trade. Public endpoints, no key. The data is not
kept in this repository: it goes to $AIMM_DATA (default ../aimm-data next to the repository).

    python3 demo/fetch_polymarket.py
    AIMM_DATA=../aimm-data/nov2 python3 demo/fetch_polymarket.py nyc=highest-temperature-in-nyc-on-november-2-2026 london=highest-temperature-in-london-on-november-2-2026
"""
import datetime, json, os, sys, time, urllib.request

EVENTS = {"nyc": "highest-temperature-in-nyc-on-feb-22", "london": "highest-temperature-in-london-on-march-17"}
OUT = os.environ.get("AIMM_DATA", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "aimm-data"))


def get(url):
    for k in range(6):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
            return json.load(urllib.request.urlopen(req, timeout=90))
        except Exception as e:
            err = e; time.sleep(2 ** k)
    raise err


def main():
    os.makedirs(OUT, exist_ok=True)
    events = dict(a.split("=", 1) for a in sys.argv[1:] if "=" in a) or EVENTS
    for key, slug in events.items():
        path = os.path.join(OUT, f"{key}.json")
        if os.path.exists(path) and "--force" not in sys.argv:
            print("have", path); continue
        e = get(f"https://gamma-api.polymarket.com/events?slug={slug}")[0]
        ts = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp())
        a, b = ts(e["startDate"]) - 2 * 86400, ts(e["endDate"]) + 2 * 86400
        ranges = []
        for m in e["markets"]:
            yes = json.loads(m["clobTokenIds"])[0]
            hist = get(f"https://clob.polymarket.com/prices-history?market={yes}&startTs={a}&endTs={b}&fidelity=1").get("history", [])
            # Data API v1 was retired in 2026; v2 pages with a cursor
            trades, cursor = [], ""
            while True:
                page = get(f"https://data-api.polymarket.com/v2/trades?conditionId={m['conditionId']}&limit=500&taker_only=true{cursor}")
                trades += page["data"]
                nxt = page.get("pagination", {}).get("next_cursor")
                if not nxt or len(trades) >= 20000:
                    break
                cursor = f"&cursor={nxt}"
            ranges.append(dict(label=m.get("groupItemTitle") or m["question"], won=json.loads(m["outcomePrices"])[0] == "1",
                               yes_token=yes, history=[[p["t"], p["p"]] for p in hist],
                               trades=[dict(t=t["timestamp"], side=t["side"], outcome=t["outcome"], size=t["size"], price=t["price"])
                                       for t in trades]))
            print(f"{key}: {ranges[-1]['label']}: {len(hist)} prices, {len(trades)} trades")
        json.dump(dict(title=e["title"], end=e["endDate"], ranges=ranges), open(path, "w"))
        print("wrote", path)


if __name__ == "__main__":
    main()
