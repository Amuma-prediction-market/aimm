"""Replays one Polymarket question on NEAR testnet, AIMM market only, exactly as `core/examples/replay.rs` ran it: the
real fills, and an arbitrageur that trades AIMM toward Polymarket's price every minute. Every number is checked
against that native run:

    cargo run -p aimm-core --example replay -- ../aimm-data/nyc.replay ../aimm-data/nyc
    python3 demo/testnet_replay.py nyc

Every transaction is signed by near-cli-rs with keys from the system keychain; this script never sees a key. Each
purchase is sent with max_cost equal to the native cost and fee, and each sale with min_proceeds equal to its proceeds;
each event must equal the native run's numbers, and the signer's tUSD must move by exactly that. Every hour of market
time the chain's prices and information intervals must equal the native run's, and they are recorded next to
Polymarket's price. Once the question closes, the resolver reports the real winner, and the redemptions and the
operator's surplus must equal the native settlement. Results go to ../aimm-data/testnet/<city>.json.
"""
import base64, json, os, re, subprocess, sys, time, urllib.request
from pathlib import Path

NET = ["network-config", "testnet"]
AIMM, FT = "market.aimm-weather.testnet", "tusd.aimm-weather.testnet"
OPERATOR, RESOLVER = "aimm-weather.testnet", "aimm-weather-oracle.testnet"
# each city's demo accounts, the one that copies Polymarket's trades and the arbitrageur, named in the environment
ACCOUNTS = {c: (os.environ.get(f"AIMM_{c.upper()}_TRADER", ""), os.environ.get(f"AIMM_{c.upper()}_ARBITRAGEUR", ""))
            for c in ("nyc", "london")}
DATA = Path(os.environ.get("AIMM_DATA", Path(__file__).resolve().parent.parent.parent / "aimm-data"))
# near-cli sends through FastNEAR's endpoint; reading from a different provider keeps the two off one rate limit
RPC = "https://rpc.testnet.near.org"
CHECK_EVERY = 25  # transactions between balance checks; each checks the running total, so no payment can slip by
FLOOR = 1.0  # NEAR: stop before any signer falls below this, so nobody runs out of gas mid-question
SECONDS_PER_TX = 18  # measured: about 13 s a trade through near-cli, with the balance check, plus the pause
PACE = 2  # seconds between transactions, to stay under the RPC services' rate limits


def rpc(params):
    body = {"jsonrpc": "2.0", "id": "r", "method": "query", "params": {"finality": "optimistic", **params}}
    # the public RPC rate-limits (HTTP 429), so back off for up to about ten minutes before giving up
    for attempt in range(10):
        try:
            req = urllib.request.Request(RPC, json.dumps(body).encode(), {"content-type": "application/json"})
            return json.load(urllib.request.urlopen(req, timeout=30))["result"]
        except Exception:
            if attempt == 9:
                raise
            time.sleep(min(2 ** attempt, 120))


def view(contract, method, args):
    r = rpc({"request_type": "call_function", "account_id": contract, "method_name": method,
             "args_base64": base64.b64encode(json.dumps(args).encode()).decode()})
    return json.loads(bytes(r["result"]).decode())


def near_left(who):
    return int(rpc({"request_type": "view_account", "account_id": who})["amount"]) / 1e24


def cash(who):
    return int(view(FT, "ft_balance_of", {"account_id": who}))


SENT = {}


def call(signer, contract, method, args, deposit="1 yoctoNEAR", gas="100 Tgas"):
    """Sends one transaction through near-cli and returns (transaction id, AIMM events by name). Raises if it failed."""
    SENT[signer] = SENT.get(signer, 0) + 1
    # a transaction burns about 0.001 NEAR, so checking every 20 keeps well clear of the floor
    if SENT[signer] % 20 == 1 and near_left(signer) < FLOOR:
        raise RuntimeError(f"{signer} is below {FLOOR} NEAR; stopping before it runs out of gas")
    cmd = ["near", "contract", "call-function", "as-transaction", contract, method, "json-args", json.dumps(args),
           "prepaid-gas", gas, "attached-deposit", deposit, "sign-as", signer, *NET, "sign-with-keychain", "send"]
    for attempt in range(9):
        res = subprocess.run(cmd, capture_output=True, text=True)
        text = res.stdout + res.stderr
        tx = re.search(r"Transaction ID: (\w+)", text)
        if tx:
            break
        # nothing was sent: a network error, a rate limit, or a nonce clash with another signer of the same account
        if any(s in text for s in ("error sending request", "timed out", "Timeout", "nonce", "rate limit", "Too Many", "429")):
            time.sleep(min(10 * 2 ** attempt, 300))
            continue
        raise RuntimeError(f"{method} by {signer} failed: {text[-800:]}")
    if not tx:
        raise RuntimeError(f"{method} by {signer}: no transaction after 9 tries")
    time.sleep(PACE)
    if "panicked" in text or "Error:" in text:
        raise RuntimeError(f"{method} by {signer} failed in {tx.group(1)}: {text[-800:]}")
    events = {}
    for m in re.finditer(r"EVENT_JSON:", text):
        try:
            v, _ = json.JSONDecoder().raw_decode(text, m.end())
        except ValueError:
            continue
        if v.get("standard") == "aimm":
            events[v["event"]] = v["data"][0]
    return tx.group(1), events


def wait_cash(who, want):
    for _ in range(15):
        time.sleep(1)
        got = cash(who)
        if got == want:
            return got
        time.sleep(1)
    raise AssertionError(f"{who} holds {got}, not {want}")


def snapshots(city):
    """The native run's rows for AIMM: time -> [(Polymarket, price, lower, upper)] per answer."""
    out = {}
    for line in (DATA / f"{city}.csv").read_text().split("\n")[1:]:
        f = line.split(",")
        if len(f) > 6 and f[1] == "AIMM":
            out.setdefault(int(f[0]), []).append(tuple(float(x) for x in (f[3], f[4], f[5], f[6])))
    return out


def main():
    city = sys.argv[1]
    trader, arb = ACCOUNTS[city]
    ops = (DATA / f"{city}.ops").read_text().split("\n")
    header = (DATA / f"{city}.replay").read_text().split("\n")[0]
    title = header.lstrip("# ").split(" ranges:")[0] + " (replay of Polymarket, with an arbitrageur)"
    answers = [a.strip() for a in header.split("ranges: ")[1].split(" winner:")[0].split("|")]
    open_line = next(l.split() for l in ops if l.startswith("open 0 "))
    depth, fund, fee, prices = open_line[2], open_line[3], open_line[4], open_line[5:]
    settle = next(l.split() for l in ops if l.startswith("settle 0 "))
    trades = [l.split() for l in ops if l.startswith("0 ")]
    snaps = snapshots(city)
    times = sorted(snaps)
    hourly = times[::60] + [times[-1]]
    out = DATA / "testnet"
    out.mkdir(exist_ok=True)
    record = {"city": city, "title": title, "answers": answers, "contract": AIMM, "token": FT, "trader": trader, "arbitrageur": arb,
              "hours": [], "txs": []}

    # RESUME=<question> picks up a run that stopped: the chain's token supply says how many transactions went through
    resume = os.environ.get("RESUME")
    start = 1
    if resume:
        qid = int(resume)
        q = view(AIMM, "get_question", {"question_id": qid})
        chain = ([int(x) for x in q["yes_supply"]], [int(x) for x in q["no_supply"]])
        yes, no = [0] * len(answers), [0] * len(answers)
        matches = []
        for i in range(len(trades) + 1):
            if (yes, no) == chain:
                matches.append(i)
            if i < len(trades):
                f = trades[i]
                (yes if f[4] == "1" else no)[int(f[3])] += (-1 if f[1].endswith("_sell") else 1) * int(f[5])
        assert len(matches) == 1, f"question {qid}: the chain's supply matches {matches} transactions done"
        start = matches[0] + 1
        q = view(AIMM, "get_question", {"question_id": qid})
        assert q["title"] == title, f"question {qid} is not {city}"
        close = int(q["close_time"])
        old = out / f"{city}.json"
        if old.exists():
            kept = json.loads(old.read_text())
            if kept.get("question") == qid:
                record = kept
        record["txs"] = [x for x in record["txs"] if True][: start - 1] if len(record["txs"]) >= start - 1 else record["txs"]
        record.update(question=qid, close_time=close)
        print(f"{city}: resuming question {qid} at transaction {start}; it closes at {time.ctime(close / 1e9)}", flush=True)
    else:
        close = (int(time.time()) + 600 + len(trades) * SECONDS_PER_TX) * 1_000_000_000
        print(f"{city}: {len(trades)} transactions; the question closes at {time.ctime(close / 1e9)}", flush=True)
        tx, _ = call(OPERATOR, AIMM, "create_question", {"args": {"collateral": FT, "title": title, "answers": answers,
            "resolver": RESOLVER, "fallback_resolver": RESOLVER, "close_time": str(close), "resolve_after": str(close),
            "fallback_after": str(close + 3_600_000_000_000), "fee_rate": fee}}, deposit="0.3 NEAR")
        qid = max(i for i in range(1, view(AIMM, "question_count", {}) + 1) if view(AIMM, "get_question", {"question_id": i})["title"] == title)
        msg = json.dumps({"action": "open", "question_id": qid, "depth": depth, "prices": prices})
        tx2, ev = call(OPERATOR, FT, "ft_transfer_call", {"receiver_id": AIMM, "amount": fund, "msg": msg})
        assert "open" in ev, f"open did not log: {tx2}"
        record.update(question=qid, create=tx, open=tx2, close_time=close)
        print(f"{city}: question {qid} opened, {tx2}", flush=True)

    def check(t):
        q = view(AIMM, "get_question", {"question_id": qid})
        row = {"time": t, "polymarket": [], "price": [], "interval": []}
        for i, (pm, p, lo, hi) in enumerate(snaps[t]):
            chain = (int(q["prices"][i]) / 1e18, int(q["intervals"][i][0]) / 1e18, int(q["intervals"][i][1]) / 1e18)
            assert chain == (p, lo, hi), f"{city} at {t}, answer {i}: chain {chain}, native {(p, lo, hi)}"
            row["polymarket"].append(pm)
            row["price"].append(chain[0])
            row["interval"].append([chain[1], chain[2]])
        record["hours"].append(row)
        inside = sum(lo - 1e-9 <= pm <= hi + 1e-9 for pm, (lo, hi) in zip(row["polymarket"], row["interval"]))
        print(f"{city}: hour {len(record['hours'])} at market time {t}: chain equals the native run; Polymarket inside AIMM's interval on {inside} of {len(answers)} answers", flush=True)
        (out / f"{city}.json").write_text(json.dumps(record, indent=1))

    purse = {trader: cash(trader), arb: cash(arb)}
    nxt = 0
    for n, f in enumerate(trades, 1):
        kind, t, rng, yes, tickets, amount, cut = f[1], int(f[2]), int(f[3]), f[4] == "1", f[5], int(f[6]), int(f[7])
        real = kind.startswith("trade")
        while nxt < len(hourly) and (t > hourly[nxt] or (real and t == hourly[nxt])):
            if n >= start:
                check(hourly[nxt])
            nxt += 1
        if n < start:
            continue
        who = trader if real else arb
        if kind.endswith("_sell"):
            tx, ev = call(who, AIMM, "sell", {"question_id": qid, "range": rng, "yes": yes, "tickets": tickets, "min_proceeds": str(amount - cut)})
            got = ev.get("sell")
            assert got and int(got["refund"]) == amount and int(got["fee"]) == cut, f"{city} sale {n}: {got} against {amount} {cut}, {tx}"
            purse[who] += amount - cut
        else:
            msg = json.dumps({"action": "buy", "question_id": qid, "range": rng, "yes": yes, "tickets": tickets, "max_cost": str(amount + cut)})
            tx, ev = call(who, FT, "ft_transfer_call", {"receiver_id": AIMM, "amount": str(amount + cut), "msg": msg})
            got = ev.get("buy")
            assert got and int(got["cost"]) == amount and int(got["fee"]) == cut, f"{city} purchase {n}: {got} against {amount} {cut}, {tx}"
            purse[who] -= amount + cut
        record["txs"].append([t, kind, tx])
        if n % CHECK_EVERY == 0 or n == len(trades):
            for w in (trader, arb):
                wait_cash(w, purse[w])
        if n % 100 == 0 or n == len(trades):
            print(f"{city}: {n} of {len(trades)} transactions exact, last {tx}", flush=True)
            (out / f"{city}.json").write_text(json.dumps(record, indent=1))
    while nxt < len(hourly):
        check(hourly[nxt])
        nxt += 1

    while time.time() * 1e9 < close + 30e9:
        time.sleep(15)
    winner, payout, redeem, surplus = int(settle[2]), settle[3], [int(settle[4]), int(settle[5])], int(settle[6])
    tx, ev = call(RESOLVER, AIMM, "resolve", {"question_id": qid, "winner": winner})
    assert ev["resolve"]["payout"] == payout and int(ev["resolve"]["surplus"]) == surplus, f"resolve {ev}, native {payout} {surplus}"
    record["resolve"] = tx
    for who, want in zip((trader, arb), redeem):
        if want == 0:
            continue
        had = cash(who)
        tx, ev = call(who, AIMM, "redeem", {"question_id": qid})
        assert int(ev["redeem"]["amount"]) == want, f"redeem by {who}: {ev}, native {want}"
        wait_cash(who, had + want)
        record.setdefault("redeem", []).append(tx)
    had = cash(OPERATOR)
    tx, ev = call(OPERATOR, AIMM, "claim_surplus", {"question_id": qid})
    assert int(ev["claim_surplus"]["amount"]) == surplus, f"surplus {ev}, native {surplus}"
    wait_cash(OPERATOR, had + surplus)
    record["claim_surplus"] = tx
    tx, _ = call(OPERATOR, AIMM, "withdraw_storage", {"question_id": qid})
    record["withdraw_storage"] = tx
    record.update(winner=winner, payout=int(payout) / 1e18, redeemed=[r / 1e6 for r in redeem], operator_gets=surplus / 1e6)
    (out / f"{city}.json").write_text(json.dumps(record, indent=1))
    print(f"{city}: settled exactly as the native run: winner {winner}, redeemed {redeem[0] / 1e6} and {redeem[1] / 1e6}, operator gets {surplus / 1e6}", flush=True)
    print(f"{city}: all checks passed", flush=True)


if __name__ == "__main__":
    main()
