//! The reference vectors carried past where `vectors.rs` stops: each holder tracked by name, every sale checked
//! against what that holder has, and then the result on every answer and every redemption. Then the same trades with a
//! full fund, which no vector has, and a vector at 6 decimals held to a few units rather than to 1e9.
#[path = "vectors_data.rs"]
#[allow(dead_code)]
mod data;
use aimm_core::math::{mul_div, mul_div_up, WAD};
use aimm_core::Question;

struct Holder {
    yes: Vec<u128>,
    no: Vec<u128>,
}

impl Holder {
    fn new(n: usize) -> Holder {
        Holder { yes: vec![0; n], no: vec![0; n] }
    }
    fn side(&mut self, range: usize, yes: bool) -> &mut u128 {
        if yes { &mut self.yes[range] } else { &mut self.no[range] }
    }
    fn winning(&self, w: usize) -> u128 {
        self.yes[w] + (0..self.no.len()).filter(|&j| j != w).map(|j| self.no[j]).sum::<u128>()
    }
}

/// Plays one vector's trades on a pot of `fund`, moving each trader's tokens, and calls `each` after every trade.
fn play(v: &data::Vector, fund: u128, mut each: impl FnMut(&Question)) -> (Question, Vec<Holder>) {
    let n = v.prices.len();
    let mut q = Question::new(n, v.fee_rate, 100, 100, 100, 0).unwrap();
    q.open(v.depth, fund, v.prices, 0).unwrap();
    let mut holders: Vec<Holder> = (0..v.traders).map(|_| Holder::new(n)).collect();
    for t in v.trades {
        let held = holders[t.who].side(t.range, t.yes);
        if t.sell {
            assert!(*held >= t.tickets, "{}: trader {} sells more than they hold", v.name, t.who);
            q.sell(t.range, t.yes, t.tickets, 0).unwrap();
            *held -= t.tickets;
        } else {
            q.buy(t.range, t.yes, t.tickets, 0).unwrap();
            *held += t.tickets;
        }
        each(&q);
    }
    (q, holders)
}

/// The result on `w`, and every holder redeems: paid at one rate, rounded down, with nothing left but the rounding.
fn settle(name: &str, q: &Question, holders: &[Holder], w: usize) {
    let mut q = q.clone();
    let (pot, owed, rate) = (q.pot, q.winning_supply(w), q.payout_if(w).unwrap());
    assert_eq!(owed, holders.iter().map(|h| h.winning(w)).sum::<u128>(), "{name}: the holders do not add up to the supply");
    assert_eq!(q.resolve(w, false, 100), Ok(rate));
    let kept = mul_div_up(owed, rate, WAD);
    assert_eq!((q.pot, q.surplus), (kept, pot - kept), "{name}: the split of the pot on {w}");
    let mut paid = 0;
    for h in holders.iter().filter(|h| h.winning(w) > 0) {
        let (_, amount) = q.redeem(&h.yes, &h.no).unwrap();
        assert_eq!(amount, mul_div(h.winning(w), rate, WAD), "{name}: one holder on {w}");
        paid += amount;
    }
    assert_eq!(q.winning_supply(w), 0, "{name}: on {w}");
    assert_eq!(q.pot, kept - paid, "{name}: on {w}");
    assert!(q.pot <= holders.len() as u128, "{name}: {} left on {w} after everyone redeemed", q.pot);
}

#[test]
fn every_vector_settles_on_every_answer() {
    for v in data::ALL {
        let (q, holders) = play(v, v.fund, |_| {});
        for w in 0..v.prices.len() {
            settle(v.name, &q, &holders, w);
        }
    }
}

#[test]
fn every_vector_at_a_full_fund_pays_every_token_in_full() {
    for v in data::ALL {
        let l = {
            let mut q = Question::new(v.prices.len(), 0, 100, 100, 100, 0).unwrap();
            q.open(v.depth, 0, v.prices, 0).unwrap();
            q.virtual_liquidity().unwrap()
        };
        let (q, holders) = play(v, l, |q| {
            let price = q.prices().unwrap();
            for i in 0..price.len() {
                assert_eq!(q.payout_if(i), Ok(WAD), "{}", v.name);
                assert_eq!(q.information_interval(i), Ok((price[i], price[i])), "{}", v.name);
            }
        });
        for w in 0..v.prices.len() {
            settle(v.name, &q, &holders, w);
        }
    }
}

struct Row {
    sell: bool,
    who: usize,
    range: usize,
    yes: bool,
    tickets: u128,
    amount: u128,
    fee: u128,
    after: [u128; 5],
}

/// The example market of `scripts/testnet.sh`, at 100 times its size and with a fee of .5% in place of its 2%, at 6
/// decimals from the reference model, by `gen_six_decimals.py`: prices .38 .22 .18 .19 .03, depth 7,130 dollars, fund
/// 5,000. It keeps exact money where the core rounds, so an amount can differ by up to two units. The prices differ by
/// about one part in the depth, since the opening book here is in whole units.
const PRICES: [u128; 5] = [38 * WAD / 100, 22 * WAD / 100, 18 * WAD / 100, 19 * WAD / 100, 3 * WAD / 100];
const SIX_DECIMALS: &[Row] = &[
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 3469151734, amount: 132807795, fee: 664039, after: [372987397990191552, 215940072520637216, 176678241153248608, 186493698995095808, 47900589340826912] },
    Row { sell: true, who: 1, range: 4, yes: true, tickets: 2221356903, amount: 92015540, fee: 460078, after: [377832146045792448, 218744926658090400, 178973121811164864, 188916073022896288, 35533732462055900] },
    Row { sell: true, who: 1, range: 4, yes: true, tickets: 662403968, amount: 22512908, fee: 112565, after: [379027032900492672, 219436703258179968, 179539120847601792, 189513516450246368, 32483626543479216] },
    Row { sell: false, who: 0, range: 4, yes: true, tickets: 102114735, amount: 3340141, fee: 16701, after: [378849514331347008, 219333929349727232, 179455033104322272, 189424757165673568, 32936766048929832] },
    Row { sell: false, who: 2, range: 1, yes: true, tickets: 6936867451, amount: 2197315892, fee: 10986579, after: [278372492918664224, 426379203342816128, 131860654540419904, 139186246459332112, 24201402738767564] },
    Row { sell: true, who: 0, range: 4, yes: true, tickets: 51826211, amount: 1249829, fee: 6249, after: [278421293554560736, 426453950585397568, 131883770631107728, 139210646777280368, 24030338451653596] },
    Row { sell: false, who: 1, range: 3, yes: false, tickets: 9887706181, amount: 9101345712, fee: 45506729, after: [310885412845166592, 476178782141937408, 147261511347710496, 38842000793565000, 26832292871620532] },
    Row { sell: false, who: 2, range: 4, yes: true, tickets: 4141047515, amount: 149081609, fee: 745408, after: [304452577235552128, 466325698980936704, 144214378690524704, 38038282781945200, 46969062311041296] },
    Row { sell: true, who: 1, range: 4, yes: true, tickets: 199560750, amount: 9249221, fee: 46246, after: [304847777281951936, 466931021292700544, 144401578712503552, 38087659046902112, 45731963665941888] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 1372976553, amount: 68907747, fee: 344539, after: [301915772811303680, 462440111586482304, 143012734489564896, 37721334622306552, 54910046490342504] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 1727249737, amount: 106518347, fee: 532592, after: [297438839085779136, 455582855629567488, 140892081672211184, 37161986849350192, 68924236763092088] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 1839564470, amount: 143201324, fee: 716007, after: [291524568054708672, 446524050486351936, 138090584868019888, 36423058258330688, 87437738332588864] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 7801641087, amount: 1142073211, fee: 5710366, after: [248376493196382048, 380434755560057216, 117652023093023104, 31032140934324176, 222504587216213408] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 1146986739, amount: 271642840, fee: 1358214, after: [239091694569070144, 366213361052272896, 113253960585349040, 29872098871399364, 251568884921908544] },
    Row { sell: true, who: 1, range: 3, yes: false, tickets: 7461349468, amount: 4787132258, fee: 23935661, after: [226586101039709760, 347058720626159744, 107330258387230960, 80614242904767408, 238410677042132160] },
    Row { sell: false, who: 2, range: 4, yes: true, tickets: 13716868455, amount: 6224274163, fee: 31121371, after: [94647104104417472, 144969628369629632, 44832838786303016, 33673312729668072, 681877116009981952] },
    Row { sell: true, who: 2, range: 4, yes: true, tickets: 15672316551, amount: 6644132154, fee: 33220661, after: [240329554062077440, 368109372899366592, 113840315082036688, 85503854647110080, 192216903309409152] },
    Row { sell: false, who: 0, range: 4, yes: true, tickets: 7450332845, amount: 2162381668, fee: 10811908, after: [177457614178620448, 271809313367412544, 84058869874083376, 63135431295446992, 403538771284436608] },
    Row { sell: true, who: 1, range: 4, yes: true, tickets: 11379676169, amount: 2768056042, fee: 13840280, after: [261637103083474656, 400745843844022272, 123933364618488016, 93084601765465424, 120599086688549616] },
    Row { sell: true, who: 0, range: 4, yes: true, tickets: 6126156764, amount: 513793809, fee: 2568969, after: [281186818715763520, 430689866292064320, 133193756233782736, 100039951266025776, 54889607492363632] },
    Row { sell: false, who: 0, range: 4, yes: true, tickets: 12272578350, amount: 1602578737, fee: 8012894, after: [224584871471897248, 343993465651131328, 106382307539319744, 79902250396190848, 245137104941460864] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 4117080846, amount: 1249650136, fee: 6248251, after: [188479110989657856, 288690783877175040, 89279578889837920, 67056632185625936, 366493894057703232] },
    Row { sell: true, who: 0, range: 4, yes: true, tickets: 7933718442, amount: 2013635703, fee: 10068179, after: [249985904272848896, 382899867703287296, 118414375708191584, 88939367054502656, 159760485261169472] },
    Row { sell: true, who: 0, range: 4, yes: true, tickets: 4078386324, amount: 514307215, fee: 2571536, after: [268684380114469440, 411540058224570368, 127271548475274992, 95591864566612416, 96912148619072736] },
    Row { sell: false, who: 0, range: 1, yes: true, tickets: 2049817864, amount: 915918471, fee: 4579592, after: [236294183510824736, 482479549382765312, 111928823768285424, 84068160487862320, 85229282850262288] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 6964945256, amount: 941397660, fee: 4706988, after: [207067377744310944, 422802515159537344, 98084547352568352, 73669919781212416, 198375639962370976] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 6096830036, amount: 1693651923, fee: 8468260, after: [163286348289242400, 333407799432108736, 77346164979114832, 58093613348837112, 367866073950696960] },
    Row { sell: false, who: 2, range: 0, yes: false, tickets: 13860415857, amount: 12785760538, fee: 63928803, after: [27174284422910580, 387644781937473600, 89928421914839392, 67543969028101624, 427708542696674880] },
    Row { sell: true, who: 2, range: 4, yes: true, tickets: 960976943, amount: 395280521, fee: 1976403, after: [28723343762232152, 409742319464285056, 95054756050070016, 71394286276919936, 395085294446492864] },
    Row { sell: false, who: 0, range: 0, yes: true, tickets: 5507148691, amount: 234671435, fee: 1173357, after: [60170851440822088, 396475888468704704, 91977120907113056, 69082717939019624, 382293421244340480] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 13025532775, amount: 7817440440, fee: 39087202, after: [20101065643791564, 132449311742382288, 30726474710087196, 23078221678639392, 793644926225099648] },
    Row { sell: false, who: 2, range: 3, yes: false, tickets: 14957886522, amount: 14812050140, fee: 74060251, after: [20516444623963720, 135186313899929472, 31361422725059908, 2890596844309590, 810045221906737280] },
    Row { sell: false, who: 2, range: 4, yes: true, tickets: 10602786880, amount: 9468993408, fee: 47344967, after: [5436726726253406, 35823509349420356, 8310576624154064, 765989692966867, 949663197607205248] },
    Row { sell: false, who: 0, range: 4, yes: true, tickets: 11118995998, amount: 10829765127, fee: 54148826, after: [1190384108435141, 7843641658868989, 1819620305275658, 167715245522600, 988978638681897728] },
    Row { sell: true, who: 0, range: 4, yes: true, tickets: 7541982906, amount: 6046599336, fee: 30232997, after: [3358684912404258, 22130941358567540, 5134083378908601, 473210840707073, 968903079509412608] },
    Row { sell: true, who: 0, range: 0, yes: true, tickets: 3546120318, amount: 9390246, fee: 46951, after: [2045238541779669, 22160107121782324, 5140849447180533, 473834472350532, 970179970416906880] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 9058377541, amount: 8903778790, fee: 44518894, after: [586687833821274, 6356747625738048, 1474680710614419, 135922003470208, 991445961826355968] },
    Row { sell: true, who: 2, range: 3, yes: false, tickets: 9794526940, amount: 8029267996, fee: 40146340, after: [586452686816557, 6354199813975529, 1474089652249093, 536671822217994, 991048586024740864] },
    Row { sell: true, who: 1, range: 3, yes: false, tickets: 2121441180, amount: 1739095709, fee: 8695479, after: [586343640077278, 6353018295353454, 1473815555678993, 722514976178226, 990864307532711936] },
    Row { sell: true, who: 1, range: 4, yes: true, tickets: 26387097057, amount: 21631373849, fee: 108156869, after: [17444259196172776, 189008100791486688, 43847359813141832, 21495480902476336, 728204799296722432] },
    Row { sell: false, who: 0, range: 4, yes: true, tickets: 3043128417, amount: 2335943038, fee: 11679715, after: [12570953365438114, 136205957158736656, 31597966368759520, 15490407758443584, 804134715348622080] },
    Row { sell: false, who: 0, range: 4, yes: true, tickets: 8594561072, amount: 7542452858, fee: 37712264, after: [4364664559303414, 47291028507925592, 10970888201284750, 5378305987382127, 931995112744104192] },
    Row { sell: true, who: 1, range: 3, yes: false, tickets: 127790385, amount: 105436415, fee: 527182, after: [4364240076097142, 47286429243277360, 10969821233196812, 5475037408540820, 931904472038887808] },
    Row { sell: false, who: 1, range: 2, yes: true, tickets: 7672086391, amount: 149609314, fee: 748047, after: [4273618985477434, 46304552051642776, 31506499301513280, 5361351210601552, 912553978450765056] },
    Row { sell: false, who: 1, range: 4, yes: true, tickets: 8010319169, amount: 7576630885, fee: 37883154, after: [1476714855076682, 16000167564000404, 10886818808160556, 1852572024525997, 969783726748236288] },
    Row { sell: true, who: 0, range: 1, yes: true, tickets: 1450962332, amount: 21036847, fee: 105184, after: [1481078290944497, 13092618995626448, 10918987466524502, 1858046053037146, 972649269193867392] },
    Row { sell: false, who: 1, range: 1, yes: true, tickets: 2531566719, amount: 39681278, fee: 198406, after: [1472858397415049, 18569894332711696, 10858387756858258, 1847734011585888, 967251125501429120] },
    Row { sell: true, who: 2, range: 0, yes: false, tickets: 6253203446, amount: 5258463326, fee: 26292317, after: [3533068481754364, 18531580017581548, 10835984199634524, 1843921676312020, 965255445624717568] },
];
// pot 55000835704 revenue 856024468 payouts [1000000000000000000, 1000000000000000000, 1000000000000000000, 1000000000000000000, 840923115890138368] winning [7301513103, 23014986843, 20619783532, 7607212411, 65405308363]
const POT: u128 = 55_000_835_704;
const REVENUE: u128 = 856_024_468;
const WINNING: [u128; 5] = [7_301_513_103, 23_014_986_843, 20_619_783_532, 7_607_212_411, 65_405_308_363];

#[test]
fn six_decimals_match_the_reference_model_to_the_unit() {
    let mut q = Question::new(5, 5 * WAD / 1000, 100, 100, 100, 0).unwrap();
    q.open(7_130_000_000, 5_000_000_000, &PRICES, 0).unwrap();
    let mut holders: Vec<Holder> = (0..3).map(|_| Holder::new(5)).collect();
    let mut capped = 0;
    for (k, t) in SIX_DECIMALS.iter().enumerate() {
        let curve = q.pool.as_ref().unwrap().sell_refund(t.range, t.yes, t.tickets);
        let held = holders[t.who].side(t.range, t.yes);
        let f = if t.sell { q.sell(t.range, t.yes, t.tickets, 0) } else { q.buy(t.range, t.yes, t.tickets, 0) }.unwrap();
        if t.sell {
            *held -= t.tickets;
            capped += (f.amount < curve) as u32;
        } else {
            *held += t.tickets;
        }
        // a capped amount is a share of the pot, and the core's pot carries up to 2 units of rounding from each trade before
        let allowed = if t.sell && f.amount < curve { 2 + 2 * k as u128 } else { 2 };
        assert!(f.amount.abs_diff(t.amount) <= allowed, "trade {k} amount: got {}, want {}", f.amount, t.amount);
        assert!(f.fee.abs_diff(t.fee) <= allowed.div_ceil(100) + 1, "trade {k} fee: got {}, want {}", f.fee, t.fee);
        for (i, (&got, &want)) in q.prices().unwrap().iter().zip(t.after.iter()).enumerate() {
            assert!(got.abs_diff(want) <= 400_000_000, "trade {k} price {i}: got {got}, want {want}");
        }
    }
    assert!(capped >= 3, "only {capped} sales were capped");
    let trades = SIX_DECIMALS.len() as u128;
    assert!(q.pot.abs_diff(POT) <= 2 * trades, "pot {}", q.pot);
    assert!(q.fees_paid.abs_diff(REVENUE) <= trades, "revenue {}", q.fees_paid);
    for w in 0..5 {
        assert!(q.winning_supply(w).abs_diff(WINNING[w]) <= 1, "winning {w}: {}", q.winning_supply(w));
        settle("six decimals", &q, &holders, w);
    }
    assert!(q.payout_if(4).unwrap() < WAD && q.payout_if(0) == Ok(WAD), "answer 4 and only answer 4 is short");
}
