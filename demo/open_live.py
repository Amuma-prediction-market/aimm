"""Opens a live testnet question that mirrors a Polymarket temperature question, for people to trade on the web app.

    AIMM_DATA=../aimm-data/nov9 python3 demo/open_live.py nyc.7.json:America/New_York london.7.json:Europe/London

Each argument is a Polymarket event as the Gamma API returns it (`/events?slug=...`), saved beforehand, and the time
zone of the city. The question takes the event's title and answers, opens at Polymarket's prices (the bid-ask midpoint
where both sides quote, floored at 1% and normalised so they add up to exactly 1), and closes at the end of that local
day, when Polymarket's last reading is in. The operator puts up $1,000 against L = $3,333, with no fee, as in the
replays. The resolver reports Polymarket's winner after the close.
"""
import json, math, sys
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo

import testnet_replay as t

WAD = 10**18
FUND = 1_000_000_000  # $1,000 of tUSD, 6 decimals
VIRTUAL = 3_333_333_333  # L, the depth the curve prices at
FLOOR = 0.01


def opening_prices(markets):
    raw = []
    for m in markets:
        bid, ask = m.get("bestBid"), m.get("bestAsk")
        raw.append((float(bid) + float(ask)) / 2 if bid is not None and ask is not None else float(json.loads(m["outcomePrices"])[0]))
    floored = [max(p, FLOOR) for p in raw]
    wad = [int(p / sum(floored) * WAD) for p in floored]
    wad[wad.index(max(wad))] += WAD - sum(wad)  # the rounding goes to the likeliest answer, so the sum is exactly 1
    return wad


def main():
    for arg in sys.argv[1:]:
        name, zone = arg.split(":")
        event = json.loads((t.DATA / name).read_text())[0]
        markets = sorted(event["markets"], key=lambda m: int(m["groupItemThreshold"]))
        answers = [m["groupItemTitle"] for m in markets]
        prices = opening_prices(markets)
        # the curve's worst loss is depth × ln(1 / lowest price); pick the depth that makes it L
        depth = int(VIRTUAL / math.log(WAD / min(prices)))
        day = datetime.fromisoformat(event["endDate"].replace("Z", "+00:00")).astimezone(ZoneInfo(zone)).date()
        close = int(datetime(day.year, day.month, day.day, tzinfo=ZoneInfo(zone)).timestamp() + 86_400) * 10**9
        title = event["title"]
        args = {"collateral": t.FT, "title": title, "answers": answers, "resolver": t.RESOLVER,
                "fallback_resolver": t.RESOLVER, "close_time": str(close), "resolve_after": str(close),
                "fallback_after": str(close + 3 * 86_400 * 10**9), "fee_rate": "0"}
        # half a NEAR pays the question's own rows and leaves storage for about 200 holders
        tx, ev = t.call(t.OPERATOR, t.AIMM, "create_question", {"args": args}, deposit="0.5 NEAR")
        qid = int(ev["create"]["id"])
        msg = json.dumps({"action": "open", "question_id": qid, "depth": str(depth), "prices": [str(p) for p in prices]})
        tx2, ev = t.call(t.OPERATOR, t.FT, "ft_transfer_call", {"receiver_id": t.AIMM, "amount": str(FUND), "msg": msg})
        assert "open" in ev, f"open did not log: {tx2}"
        q = t.view(t.AIMM, "get_question", {"question_id": qid})
        print(f"question {qid}: {title}, closes {datetime.fromtimestamp(close / 1e9, ZoneInfo(zone))}, "
              f"L = {int(q['virtual_liquidity']) / 1e6:.2f}, created {tx}, opened {tx2}")
        print("  " + ", ".join(f"{a} {int(p) / 1e16:.2f}%" for a, p in zip(answers, q["prices"])))


if __name__ == "__main__":
    main()
