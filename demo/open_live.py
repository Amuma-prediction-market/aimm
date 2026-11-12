"""Opens a live testnet question that mirrors a Polymarket temperature question, for people to trade on the web app.

    AIMM_DATA=../aimm-data/nov9 python3 demo/open_live.py nyc.7.json:America/New_York london.7.json:Europe/London
    AIMM_DATA=../aimm-data/nov9 python3 demo/open_live.py --force nyc.7.json:America/New_York

Each argument is a Polymarket event as the Gamma API returns it (`/events?slug=...`), saved beforehand, and the time
zone of the city. The question takes the event's title and answers, opens inside Polymarket's bid-ask spread on every
answer (see opening_prices), and closes at the end of that local day, when Polymarket's last reading is in. The operator puts up $1,000 against L = $3,333, with no fee, as in the
replays, unless FUND and DEPTH (dollars) are set. The resolver reports Polymarket's winner after the close.

The question's storage NEAR covers HOLDERS holders (400 unless set; see storage_deposit), and settle_live.py takes back
what is left. If a question needs more, `python3 demo/settle_live.py --top-up 12 13` adds room for HOLDERS more holders
to questions 12 and 13. The script will not open a second unresolved question with the same title, so a rerun cannot
fund a market twice; --force opens it anyway.
"""
import json, math, os, sys
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo

import testnet_replay as t

WAD = 10**18
# the operator's fund and the depth the curve prices at, in tUSD's smallest unit (6 decimals): $1,000 against $3,333
# unless FUND and DEPTH (in dollars) say otherwise
FUND = int(float(os.environ.get("FUND", 1_000)) * 1e6)
VIRTUAL = int(float(os.environ.get("DEPTH", 3_333.333333)) * 1e6)
FLOOR = 0.001  # Polymarket's tick, so the curve never sees a zero
BYTE_COST = 10**19  # yoctoNEAR a byte of contract storage costs on NEAR
HOLDERS = int(os.environ.get("HOLDERS", 400))  # how many holders a question's storage NEAR should cover
SCAN = 30  # how many of the newest questions to look through for one with the same title


def holder_row_bytes(n):
    """A holder's row with n answers: the key (prefix, question id, a 42-character 0x account name), a YES and a NO
    amount on every answer, and NEAR's 40 bytes for a record. 451 bytes, about 0.0045 NEAR, with 11 answers."""
    return 1 + 8 + 4 + 42 + 4 + 32 * n + 40


def storage_deposit(n, holders=HOLDERS):
    """yoctoNEAR to attach when creating: rows for `holders` holders, plus the question's own row and the pool the open
    adds (about 1,300 bytes with 11 answers; 2,000 + 64 n leaves room for long titles and labels). About 1.83 NEAR for
    400 holders on 11 answers; whatever is not used comes back when the question is settled."""
    return (holders * holder_row_bytes(n) + 2_000 + 64 * n) * BYTE_COST


def num(x):
    return None if x is None or x == "" else float(x)


def opening_prices(markets):
    """Prices inside Polymarket's bid-ask spread on every answer, adding up to exactly 1: the midpoints, moved toward
    the bids (or asks) in proportion to the room each answer has there when they do not add up to 1. An arbitrageur keeps
    them there with the same rule, so AIMM never sits below or above Polymarket's quotes for long."""
    rows = []
    for m in markets:
        bid, ask = num(m.get("bestBid")), num(m.get("bestAsk"))
        mid = (bid + ask) / 2 if bid is not None and ask is not None else float(json.loads(m["outcomePrices"])[0])
        mid = max(mid, FLOOR)
        # an empty side of the book is its extreme: nobody bids means a bid of 0 (floored at the tick), nobody offers
        # means an ask of 1, so that answer has its whole range as room and the others can stay inside their spreads
        rows.append((mid, min(mid, max(bid if bid is not None else 0, FLOOR)), max(mid, ask if ask is not None else 1)))
    excess = sum(r[0] for r in rows) - 1
    room = [r[0] - r[1] if excess > 0 else r[2] - r[0] for r in rows]
    if sum(room) >= abs(excess) and sum(room) > 0:
        prices = [r[0] - excess * k / sum(room) for r, k in zip(rows, room)]
    else:
        # the spreads together cannot absorb the difference, so every price is scaled by the same factor instead
        print(f"note: the spreads leave {sum(room):.4f} of room for a difference of {excess:+.4f}; opening at the "
              "midpoints scaled to add up to 1, which puts some answers outside Polymarket's spread", file=sys.stderr)
        prices = [r[0] / (excess + 1) for r in rows]
    wad = [int(p * WAD) for p in prices]
    wad[wad.index(max(wad))] += WAD - sum(wad)  # the rounding goes to the likeliest answer, so the sum is exactly 1
    return wad


def unresolved_with_title(title):
    """Ids of the newest questions that carry this title and are not resolved yet, opened or not."""
    count = t.view(t.AIMM, "question_count", {})
    found = []
    for i in range(count, max(count - SCAN, 0), -1):
        q = t.view(t.AIMM, "get_question", {"question_id": i})
        if q["title"] == title and not q["resolved"]:
            found.append(i)
    return found


def main():
    force = "--force" in sys.argv[1:]
    specs = [a for a in sys.argv[1:] if a != "--force"]
    events = [(json.loads((t.DATA / name).read_text())[0], zone) for name, zone in (a.split(":") for a in specs)]
    # a rerun on the same file would create and fund a second copy of a market people already trade, so check every
    # argument before creating anything
    if not force:
        for event, _ in events:
            same = unresolved_with_title(event["title"])
            if same:
                sys.exit(f"{event['title']} is already question {', '.join(map(str, same))}, not yet resolved; "
                         "nothing was created. Run with --force to open another one.")
    for event, zone in events:
        markets = sorted(event["markets"], key=lambda m: int(m["groupItemThreshold"]))
        answers = [m["groupItemTitle"] for m in markets]
        prices = opening_prices(markets)
        # the curve's worst loss is depth × ln(1 / lowest price); pick the depth that makes it L
        depth = int(VIRTUAL / math.log(WAD / min(prices)))
        day = datetime.fromisoformat(event["endDate"].replace("Z", "+00:00")).astimezone(ZoneInfo(zone)).date()
        # the next local midnight, not midnight + 24 hours: a clock-change day lasts 23 or 25 hours
        nxt = day + timedelta(days=1)
        close = int(datetime(nxt.year, nxt.month, nxt.day, tzinfo=ZoneInfo(zone)).timestamp()) * 10**9
        title = event["title"]
        args = {"collateral": t.FT, "title": title, "answers": answers, "resolver": t.RESOLVER,
                "fallback_resolver": t.RESOLVER, "close_time": str(close), "resolve_after": str(close),
                "fallback_after": str(close + 3 * 86_400 * 10**9), "fee_rate": "0"}
        # pays the question's own rows and leaves storage for HOLDERS holders: about 1.83 NEAR for 400 on 11 answers
        before = t.view(t.AIMM, "question_count", {})

        def created():
            # if near-cli loses the reply after sending, the question may be on chain already; a blind retry would
            # create a second one
            return next((i for i in range(before + 1, t.view(t.AIMM, "question_count", {}) + 1)
                         if t.view(t.AIMM, "get_question", {"question_id": i})["title"] == title), None)

        tx, ev = t.call(t.OPERATOR, t.AIMM, "create_question", {"args": args},
                        deposit=f"{storage_deposit(len(answers))} yoctoNEAR", landed=lambda: created() is not None)
        qid = int(ev["create"]["id"]) if ev else created()
        msg = json.dumps({"action": "open", "question_id": qid, "depth": str(depth), "prices": [str(p) for p in prices]})
        tx2, ev = t.call(t.OPERATOR, t.FT, "ft_transfer_call", {"receiver_id": t.AIMM, "amount": str(FUND), "msg": msg},
                         landed=lambda: t.view(t.AIMM, "get_question", {"question_id": qid})["opened"])
        assert ev is None or "open" in ev, f"open did not log: {tx2}"
        q = t.view(t.AIMM, "get_question", {"question_id": qid})
        print(f"question {qid}: {title}, closes {datetime.fromtimestamp(close / 1e9, ZoneInfo(zone))}, "
              f"L = {int(q['virtual_liquidity']) / 1e6:.2f}, created {tx}, opened {tx2}")
        print("  " + ", ".join(f"{a} {int(p) / 1e16:.2f}%" for a, p in zip(answers, q["prices"])))


if __name__ == "__main__":
    main()
