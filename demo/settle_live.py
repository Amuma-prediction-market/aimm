"""Settles closed live testnet questions: the resolver reports the winner, the operator redeems for every account the
demo trades from (redeem pays only the holder), claims the surplus and takes back the storage NEAR.

    python3 demo/settle_live.py 10:4 11:5
    python3 demo/settle_live.py 12:6:live/london.17.json
    python3 demo/settle_live.py --top-up 12 13

Each argument is a question and its winning answer's index, as Polymarket resolved it, counted from 0 in the order the
question lists its answers. Before reporting, the script shows the answer that index names and asks for a yes (--yes
skips the question). A third part names the Polymarket event saved after it resolved (under AIMM_DATA, as for
open_live.py); the script then checks that Polymarket has resolved it, that it is the same question, and that its winner
is the answer typed. Every step looks at the chain first and skips what is already done, so the script can be run again
on the same question, for example to take back storage NEAR that later redemptions freed. Accounts outside the demo
redeem their own tokens on the web app.

--top-up adds room for HOLDERS more holders (400 unless set) to each open question listed, through add_storage.
"""
import json, sys

import open_live
import testnet_replay as t

HOLDERS = ["aimm-weather-trader.testnet", "aimm-weather-nyc-bot.testnet", "aimm-weather-london-trader.testnet", "aimm-weather-london-bot.testnet"]


def polymarket_winner(name, q):
    """The answer index the saved Polymarket event resolved to; exits if it has not resolved or is another question."""
    event = json.loads((t.DATA / name).read_text())
    event = event[0] if isinstance(event, list) else event
    if event["title"] != q["title"]:
        sys.exit(f"{name} is {event['title']!r}, not {q['title']!r}")
    if not event.get("closed") or any(m.get("umaResolutionStatus") != "resolved" for m in event["markets"]):
        sys.exit(f"{name}: Polymarket has not resolved every answer yet; save the event again once it has")
    won = [m["groupItemTitle"] for m in event["markets"] if json.loads(m["outcomePrices"])[0] == "1"]
    if len(won) != 1 or won[0] not in q["answers"]:
        sys.exit(f"{name}: expected one winning answer among {q['answers']}, found {won}")
    return q["answers"].index(won[0])


def settle(qid, winner, event=None, ask=True):
    q = t.view(t.AIMM, "get_question", {"question_id": qid})
    if not q["resolved"]:
        if not 0 <= winner < len(q["answers"]):
            sys.exit(f"question {qid} has answers 0 to {len(q['answers']) - 1}, not {winner}")
        if event is not None:
            poly = polymarket_winner(event, q)
            if poly != winner:
                sys.exit(f"question {qid}: Polymarket's winner is {poly} ({q['answers'][poly]}), not {winner}")
        print(f"question {qid}: {q['title']}")
        print("  " + ", ".join(f"{i} {a}" for i, a in enumerate(q["answers"])))
        # resolve is final, so the operator sees the label the index names before it goes out
        if ask and input(f"report {winner} ({q['answers'][winner]}) as the winner? This is final. [y/N] ").strip().lower() != "y":
            print(f"question {qid}: left unresolved", flush=True)
            return
        tx, _ = t.call(t.RESOLVER, t.AIMM, "resolve", {"question_id": qid, "winner": winner},
                       landed=lambda: t.view(t.AIMM, "get_question", {"question_id": qid})["resolved"])
        q = t.view(t.AIMM, "get_question", {"question_id": qid})
        print(f"question {qid}: resolved, {q['answers'][q['winner']]} won, payout {int(q['payouts'][q['winner']]) / 1e18}, {tx}", flush=True)
    elif q["winner"] != winner:
        print(f"question {qid}: already resolved with {q['winner']} ({q['answers'][q['winner']]}) as the winner; "
              "finishing its settlement", flush=True)
    for h in HOLDERS:
        def holds(h=h):
            held = t.view(t.AIMM, "holdings", {"question_id": qid, "account_id": h})
            return any(int(x) for x in held["yes"] + held["no"])
        if holds():
            tx, ev = t.call(t.OPERATOR, t.AIMM, "redeem", {"question_id": qid, "account_id": h}, landed=lambda: not holds())
            amount = f"{int(ev['redeem']['amount']) / 1e6} tUSD" if ev else "its tokens"
            print(f"question {qid}: redeemed {amount} for {h}, {tx}", flush=True)
    if q["operator"] != t.OPERATOR:
        # a question the arbitrage bot opened on its own (app/bot/src/open.ts): only its operator, the bot, can claim
        # the surplus and the storage NEAR, and the bot does that by itself after the result
        print(f"question {qid}: operated by {q['operator']}, which takes back the surplus and storage NEAR itself", flush=True)
        return
    # the claim zeroes the surplus, and the contract refuses a second claim, so a surplus of 0 means it is done
    surplus = lambda: int(t.view(t.AIMM, "get_question", {"question_id": qid})["surplus"])
    if surplus():
        tx, ev = t.call(t.OPERATOR, t.AIMM, "claim_surplus", {"question_id": qid}, landed=lambda: not surplus())
        amount = f"{int(ev['claim_surplus']['amount']) / 1e6} tUSD" if ev else "the surplus"
        print(f"question {qid}: surplus {amount} to the operator, {tx}", flush=True)
    storage = lambda: int(t.view(t.AIMM, "get_question", {"question_id": qid})["storage"])
    if storage():
        tx, _ = t.call(t.OPERATOR, t.AIMM, "withdraw_storage", {"question_id": qid}, landed=lambda: not storage())
        print(f"question {qid}: storage NEAR taken back, {tx}", flush=True)
    else:
        print(f"question {qid}: settled, no storage NEAR left to take back", flush=True)


def top_up(qid):
    """Adds room for open_live.HOLDERS more holders to the question's storage balance; anyone may call add_storage."""
    q = t.view(t.AIMM, "get_question", {"question_id": qid})
    if q["resolved"]:
        print(f"question {qid}: resolved, so it needs no more storage", flush=True)
        return
    amount = open_live.HOLDERS * open_live.holder_row_bytes(len(q["answers"])) * open_live.BYTE_COST
    before = int(q["storage"])
    # a retry after a lost reply would add the NEAR twice, so look for it on chain first
    tx, _ = t.call(t.OPERATOR, t.AIMM, "add_storage", {"question_id": qid}, deposit=f"{amount} yoctoNEAR",
                   landed=lambda: int(t.view(t.AIMM, "get_question", {"question_id": qid})["storage"]) > before)
    after = int(t.view(t.AIMM, "get_question", {"question_id": qid})["storage"])
    print(f"question {qid}: added {amount / 1e24} NEAR of storage, {after / 1e24:.4f} NEAR in all, {tx}", flush=True)


if __name__ == "__main__":
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if flags - {"--top-up", "--yes"}:
        sys.exit(f"unknown option {', '.join(sorted(flags - {'--top-up', '--yes'}))}; see the usage at the top of this file")
    if "--top-up" in flags:
        for arg in args:
            top_up(int(arg))
    else:
        for arg in args:
            qid, winner, *event = arg.split(":", 2)
            settle(int(qid), int(winner), event[0] if event else None, ask="--yes" not in flags)
