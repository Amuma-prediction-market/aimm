"""A short vector at 6 decimals from the reference model (sims/lmsr_pool.py in the AIMM repo), which the vectors of
tests/vectors/ cannot give: those are all at 18 decimals and their tolerance hides anything below 1e9 units. This is the
README market, in the smallest unit of a 6-decimal stablecoin, crowded enough on answer 4 that sales get capped. Prints
the rows of SIX_DECIMALS in replay.rs.  python3 tests/gen_six_decimals.py path/to/sims"""
import random, sys
sys.path.insert(0, sys.argv[1])
from lmsr_pool import LMSRPool

PRICES = [0.38, 0.22, 0.18, 0.19, 0.03]
DEPTH, FUND, FEE = 7_130_000_000, 5_000_000_000, 0.005
TRADERS = 3
m = LMSRPool(PRICES, DEPTH, fee=FEE, fund=FUND)
rng = random.Random(61)
for _ in range(48):
    who = rng.randrange(TRADERS)
    h = m.held.get(f"t{who}")
    sellable = [s for s in range(2 * m.n) if h and h[s] >= 1] if h else []
    if sellable and rng.random() < 0.4:
        s = rng.choice(sellable)
        i, yes = s % m.n, s < m.n
        n = max(1, int(h[s] * rng.uniform(0.3, 1.0)))
        amount, fee = m.quote_sell(i, yes, n)
        m.sell(f"t{who}", i, yes, n)
        sell = True
    else:
        i = 4 if rng.random() < 0.55 else rng.randrange(m.n)
        yes = i == 4 or rng.random() < 0.5
        n = int(rng.uniform(1e6, 1.6e10))
        amount, fee = m.quote_buy(i, yes, n)
        m.buy(f"t{who}", i, yes, n)
        sell = False
    print(f"    Row {{ sell: {str(sell).lower()}, who: {who}, range: {i}, yes: {str(yes).lower()}, tickets: {n}, amount: {round(amount)}, "
          f"fee: {round(fee)}, after: [{', '.join(str(round(p * 1e18)) for p in m.prices())}] }},")
print("// pot", round(m.pot), "revenue", round(m.revenue), "payouts", [round(m.payout(w) * 1e18) for w in range(m.n)], "winning", [round(m.winning(w)) for w in range(m.n)])
