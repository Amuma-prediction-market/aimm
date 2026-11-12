# Specification

What the contract does, exactly. Why AIMM is the better market is in `whitepaper/aimm.pdf`. The Rust core must match a Python reference model of the
curve, pot and payout trade by trade, on vectors kept in `core/tests/vectors/`.

## 1. The curve

A question has n answers, 3 to 16, exactly one of which happens. q_i counts the tokens that pay if answer i happens:
YES on i, and NO on every other answer. The curve is Hanson's with a fixed depth b:

    C(q) = b ln sum_i exp(q_i / b)

It opens at q0_i = b ln p0_i, so C(q0) = 0 and the prices p_i = exp(q_i / b) / sum_k exp(q_k / b) are the opening
prices. Buying n YES on i raises q_i by n; buying n NO on i raises every other q_j by n. Either costs the rise in C,
plus one unit; selling refunds the fall in C, less one unit. A sale so small that rounding makes C rise is refused
(`SaleTooSmall`). The virtual liquidity L = b ln(1 / min p0) is the most the curve can owe beyond what it takes in. No
money backs it.

## 2. Pot, payout and the sell cap

- **Fund.** The operator's real money ℓ, sent with the `open` message. The operator chooses it: ℓ < L is AIMM, ℓ = L
  is a plain market maker, and the contract enforces neither.
- **Pot.** The fund, plus every curve cost paid in, less every refund and redemption paid out and the surplus set
  aside at the result.
- **Payout.** If answer w happens, each of the W_w winning tokens is paid r_w = min(1, pot / W_w), whenever it was
  bought (whitepaper, Proposition 1). The operator keeps the rest of the pot.
- **Sell cap.** A sale of n YES on i refunds min(curve refund, n r_i); a sale of n NO on i refunds
  min(curve refund, n min_{j≠i} r_j), with the payouts as the pot stands before the sale. So no sale lowers the
  payout on any answer the sold token pays on. It can lower the payout on other answers, because the pot is shared.
  The cap holds per sale only: a holder with temporary capital can reach the curve refund across several trades, for
  example by buying another answer first and selling it back after. What holds is the floor: the pot stays at or above
  fund + C(q), so with exact C the payout on w is at least its opening price. Computed C can sit up to about 30 b / 1e18
  units lower, the largest found (section 4), so with a fund smaller than that a dust purchase can leave a payout just
  under the opening price. The cap cannot bind on a sale of YES on i when the fund is at least b ln(1 / p0_i), and cannot
  bind on any sale when the fund is at least L, so an operator who wants exits to be equal can size the fund at L. The
  NO cap uses the least payout, so one account can lower it for every NO seller by buying up a thin answer; no higher
  cap keeps guarantee 3, so clients should read `quote_sell` just before selling.
- **Dust.** At a short pot the result keeps ceil(r W) and each redemption pays a floor, so up to about one unit per
  redemption stays in the contract after the last winner. Nobody can claim it. Two rows redeemed apart can be paid one
  unit less than the same tokens in one row, and a stranger can redeem before a holder merges them.
- **Fee.** fee_rate (at most 0.1) times the curve's cost on a buy, or times the refund on a sale, rounded up. The
  operator's revenue. It never enters the pot.

## 3. The prediction range

For answer i with price p, let r_yes be what a YES on i would be paid as the pot stands and r_no the least of what a
NO on i could be paid. The prediction range is [max(0, 1 − (1 − p) / r_no), min(1, p / r_yes)] (whitepaper, Definition 5).
With two answers, a risk-neutral trader buys YES exactly when their belief is above the upper end and NO exactly when
it is below the lower end (Proposition 3). The whitepaper's proposition is binary. With three or more answers this contract
takes r_no as the worst case: the upper end still decides a YES, and a belief below the lower end is enough to buy NO
but not necessary, because a NO pays differently on each answer it pays on. All of this is for the next token, with no
fee and the pot as it stands; with a fee, a belief must also clear the fee. With ℓ = L the payouts are always 1 and the
range is the price itself. The contract returns it for every answer in `get_question` and `information_intervals`,
rounded outward.

## 4. Guarantees

1. On the curve, a purchase and the sale of the same tokens differ by at most two units. The sell cap can make the
   refund smaller when the pot is short, and the fees come on top.
2. A lone trader starting at the true chances has an expected profit of at most zero on any sequence of buys and sells.
3. A sale never lowers the payout on any answer the sold token pays on.

They hold for exact arithmetic. Here ln and exp are computed in
fixed point. Guarantees 1 and 3 do not depend on that: `core/tests/` checks them at 6, 18 and 24 decimals. Guarantee 2
does. A cost is floor(C after) − floor(C before) + 1, so rounding C to a unit uses up the margin and leaves nothing for
the error in ln and exp. A purchase can therefore have a positive expected value of up to about 26 b / 1e18 units, the
largest found and not a proved bound, on books chosen for it: at depth 1e17 with four equal answers, 4 NO cost 2 and are
worth 3. On random books at depths up to 1e15 no case appeared. Total cost stays at least C(end) − C(start) plus one unit
for each purchase, so the gain comes once and does not compound. A fee of any size adds at least one unit to a purchase.

## 5. Units and limits

- Amounts of tokens and of money are in the collateral's smallest unit. At payout 1 a winning token is paid one unit.
  With a 6-decimal stablecoin, 1,000,000 tokens pay 1 dollar.
- Prices, payouts, the fee rate and the prediction range are in WAD, 1e18 = 1.
- Every opening price is at least 1e-6 and they add up to exactly 1e18.
- One trade moves at most 2^100 tokens, and no answer carries more winning tokens than that; the depth is at most
  2^100. All three are in the collateral's smallest unit, about 1.27e12 whole tokens at 18 decimals and 1.27e6 at 24.
  The book is i128, and every product goes through a 256-bit multiply-divide.
- ln and exp are computed in fixed point, accurate to a few parts in 1e17. A term below e^-41 counts as 0. Cost
  resolution is about 2 b / 1e18 units, so the depth must suit the token: keep it under about 1e12 whole tokens. The
  contract does not know the token's decimals, so a depth written in WAD on a 6-decimal token is accepted and gives a
  curve that hardly moves. That risk is the operator's own fund.

## 6. The contract

One contract holds every question. Its state per question is the core `Question` plus the collateral (a NEP-141
token), the operator (whoever created it), the resolver, the fallback resolver, a title, the answer labels and the
storage balance.

| Method | Who | What |
|---|---|---|
| `create_question(args)` | anyone, attaching NEAR for storage | becomes the operator; times are NEAR block times in nanoseconds |
| `ft_on_transfer` with `{"action":"open",...}` | the operator, through the collateral token | opens the curve; the amount sent is the fund |
| `ft_on_transfer` with `{"action":"buy",...}` | anyone, through the collateral token | buys for the sender; returns what was not needed, which the token contract sends back |
| `sell(...)` | a holder, 1 yoctoNEAR | burns, pays the seller refund less fee |
| `mt_transfer(...)` | a holder, at least 1 yoctoNEAR, and the cost of the receiver's whole row if they hold nothing of this question | moves tokens; ids are `<question>:yes:<answer>` and `<question>:no:<answer>`; what was not needed comes back |
| `halt` | resolver or operator, 1 yoctoNEAR | stops trading at once and for good, saying nothing about the result |
| `resolve(question_id, winner)` | resolver once trading has stopped and from `resolve_after`; fallback resolver from `fallback_after`; 1 yoctoNEAR | fixes the payout, final |
| `redeem(question_id, account_id)` | anyone, 1 yoctoNEAR | settles the account, the caller by default: pays it floor(winning tokens × payout), drops its losing tokens, which pay nothing, and frees its row |
| `claim_fees`, `claim_surplus` | operator, 1 yoctoNEAR | pays the operator: fees at any time; the rest of the pot after the result, once, and a second claim is refused |
| `withdraw_pending(question_id, account_id)` | anyone, 1 yoctoNEAR | sends an account the money from a transfer that failed |
| `add_storage(question_id)` | anyone, attaching NEAR | adds to the question's storage balance |
| `withdraw_storage(question_id)` | operator, after the result, or after the close of a question that never opened, 1 yoctoNEAR | takes the storage balance; rows freed later fill it again, so it can be called again |

The 1 yoctoNEAR means a full-access key: a function-call key cannot make these calls.

Money comes in only through `ft_on_transfer` and goes out only through `ft_transfer`, after the state has changed, and
only to the account that earned it: a purchase goes to its sender, a sale pays the seller, a redemption the holder and a
claim the operator. If a transfer out fails, for example because that account is not registered with the token, the
amount is kept for it under `pending`. So no result depends on a transfer, and money is never lost to a failed one.
`sell`, `redeem` and the claims return the amount owed, not the amount delivered; a `payment_failed` event says when a
transfer did not arrive. Collateral must be a plain NEP-141 that settles `ft_transfer` in the receipt it is called in:
a token that takes a cut of its own transfers would leave the pot short of what it records, and one that pays first and
fails in a later receipt would be recorded as failed and could be paid twice from that token's balance here.

**Events.** NEAR keeps the logs of a failed receipt, and a purchase that fails inside `ft_on_transfer` still leaves the
transaction's status as success. So an event counts only from a receipt that went through, and clients must check each
receipt. Calls that pay make the payment before they log, so a call refused for storage logs nothing.

**Storage.** Each question pays for its own storage in NEAR from one balance. What `create_question` attaches beyond
the question's row fills it, and anyone can add to it. Opening takes 24 + 32 n bytes from it, 184 at five answers, and
is refused if the balance is short. A purchase by a new holder takes that holder's row from it, 227 bytes at five
answers with a 10-character name, about 0.0023 NEAR, and every byte freed is credited to it, so one question can never
spend what another put in. A row that `mt_transfer` adds is paid for whole by the caller: 0.01 NEAR always covers it,
and the rest comes back. The cost is not refundable: when that row goes, it is credited to the question like any other,
including a sender's row freed by the move itself. So nobody can spend a question's balance on holder rows for names of
their choosing. Each holder row it pays for belongs to an account that bought through the token, which the token only
lets a registered account do, once per account while the row lasts; a fake token can name any sender, but only on its
own question. Each payment out holds the cost of one `pending` row, about 0.001 NEAR, until its outcome is known, so a
failed transfer can always be recorded, and that row, one per account per question, stays charged to the balance until
the money is delivered. If the balance runs out, buying as a new holder and part sales stop for that question alone
until anyone adds to it; the tokens of a refused purchase go back to the buyer, and because `add_storage` and a purchase
go to different contracts, a client should add to the balance first and send the purchase right after. A redemption,
and a sale that empties a row, free that row, which always pays for their own hold, so neither needs the balance. A
unit sent to a holder can turn their full exit into a part sale; they can sell that unit first or add a little to the
balance. The operator can take the balance as soon as the result is in; each claim holds a row's cost, so claim first
or add a little back. A row has 2n slots however few positions it holds, so it costs the same for one position as for
many.

**Who is trusted.** The resolver and the fallback resolver decide the result, and a report is final: a resolver can
name any answer, including one nobody holds, which sends the whole pot to the operator. So traders must look at who
they are before buying. There is no void result: a question's terms must name an answer in every case, and if neither
resolver ever reports, the money stays in the contract. Times have no upper limit, so look at those too. A halt cannot
be undone. The contract has no owner and no upgrade method; whoever controls the keys of its account controls the code.
A question's collateral can be any contract, which can make the contract log any buyer it likes, so clients must check
`collateral` before showing a question. Nothing stops the operator trading in its own market, and its terms should
forbid it. Anyone can sandwich a trade within the slack of its `max_cost` or `min_proceeds`, so clients should set both
close to the quote. When the sell cap binds, a sandwich needs no slack at all, because the seller gains and those who
stay pay for it. Tokens sent to a name nobody controls stay there: anyone can settle that name after the result, but
what it wins stays under `pending` for it. Quotes cover what the market has issued, not what one seller holds.
