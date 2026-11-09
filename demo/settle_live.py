"""Settles closed live testnet questions: the resolver reports the winner, the operator redeems for every account the
demo trades from (redeem pays only the holder), claims the surplus and takes back the storage NEAR.

    python3 demo/settle_live.py 10:4 11:5

Each argument is a question and its winning answer's index, as Polymarket resolved it. Accounts outside the demo
redeem their own tokens on the web app.
"""
import sys

import testnet_replay as t

HOLDERS = ["aimm-weather-trader.testnet", "aimm-weather-nyc-bot.testnet", "aimm-weather-london-trader.testnet", "aimm-weather-london-bot.testnet"]


def settle(qid, winner):
    q = t.view(t.AIMM, "get_question", {"question_id": qid})
    if not q["resolved"]:
        tx, ev = t.call(t.RESOLVER, t.AIMM, "resolve", {"question_id": qid, "winner": winner})
        print(f"question {qid}: resolved, {q['answers'][winner]} won, payout {int(ev['resolve']['payout']) / 1e18}, {tx}", flush=True)
    for h in HOLDERS:
        held = t.view(t.AIMM, "holdings", {"question_id": qid, "account_id": h})
        if any(int(x) for x in held["yes"] + held["no"]):
            tx, ev = t.call(t.OPERATOR, t.AIMM, "redeem", {"question_id": qid, "account_id": h})
            print(f"question {qid}: redeemed {int(ev['redeem']['amount']) / 1e6} tUSD for {h}, {tx}", flush=True)
    tx, ev = t.call(t.OPERATOR, t.AIMM, "claim_surplus", {"question_id": qid})
    print(f"question {qid}: surplus {int(ev['claim_surplus']['amount']) / 1e6} tUSD to the operator, {tx}", flush=True)
    tx, _ = t.call(t.OPERATOR, t.AIMM, "withdraw_storage", {"question_id": qid})
    print(f"question {qid}: storage NEAR taken back, {tx}", flush=True)


if __name__ == "__main__":
    for arg in sys.argv[1:]:
        qid, winner = arg.split(":")
        settle(int(qid), int(winner))
