//! The vectors in `vectors/` were produced by a Python reference model of the curve, pot and payout, which is not in
//! this repository, and are kept as fixed data. The core must match them trade by trade: every buy's cost and every
//! sale's capped refund, every fee, every price after, and at the end the pot, the fee revenue, the winning tokens and
//! the payout on every answer, to one part in 1e9 or 1e9 wei.
#[path = "vectors_data.rs"]
#[allow(dead_code)]
mod data;
use aimm_core::Question;

fn close(name: &str, what: &str, got: u128, want: u128) {
    let diff = got.abs_diff(want);
    assert!(diff <= 1_000_000_000 || diff as f64 <= 1e-9 * want as f64, "{name}: {what}: got {got}, want {want}");
}

#[test]
fn matches_the_reference_model_trade_by_trade() {
    for v in data::ALL {
        let now = 0;
        let mut q = Question::new(v.prices.len(), v.fee_rate, 100, 100, 100, now).unwrap();
        q.open(v.depth, v.fund, v.prices, now).unwrap();
        for (k, t) in v.trades.iter().enumerate() {
            let fill = if t.sell { q.sell(t.range, t.yes, t.tickets, now) } else { q.buy(t.range, t.yes, t.tickets, now) }
                .unwrap_or_else(|e| panic!("{}: trade {k}: {e}", v.name));
            close(v.name, &format!("trade {k} amount"), fill.amount, t.amount);
            close(v.name, &format!("trade {k} fee"), fill.fee, t.fee);
            for (i, (&got, &want)) in q.prices().unwrap().iter().zip(t.after.iter()).enumerate() {
                close(v.name, &format!("trade {k} price {i}"), got, want);
            }
        }
        close(v.name, "pot", q.pot, v.pot);
        close(v.name, "revenue", q.fees_paid, v.revenue);
        for w in 0..v.prices.len() {
            close(v.name, &format!("winning {w}"), q.winning_supply(w), v.winning[w]);
            close(v.name, &format!("winning {w} on the curve"), q.pool.as_ref().unwrap().winning(w), v.winning[w]);
            close(v.name, &format!("payout {w}"), q.payout_if(w).unwrap(), v.payouts[w]);
        }
        println!("{}: {} trades match", v.name, v.trades.len());
    }
}
