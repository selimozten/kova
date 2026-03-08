use std::time::{SystemTime, UNIX_EPOCH};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::exchange::types::{OrderBook, OrderSide};

use super::types::{ArbitrageOpportunity, LegDetail, TradeLeg, TrianglePath};

/// Evaluate a triangle path against current order book state.
/// Returns Some(opportunity) if the path is profitable, None otherwise.
///
/// This is a pure function — no exchange dependency.
pub fn evaluate_triangle(
    path: &TrianglePath,
    order_books: &impl Fn(&str) -> Option<OrderBook>,
    start_amount: Decimal,
    taker_fee: Decimal,
    slippage_bps: Decimal,
) -> Option<ArbitrageOpportunity> {
    let mut current_amount = start_amount;
    let mut legs = Vec::with_capacity(3);

    for leg in &path.legs {
        let ob = order_books(&leg.symbol)?;
        let detail = evaluate_leg(leg, &ob, current_amount, taker_fee, slippage_bps)?;
        current_amount = detail.output_amount;
        legs.push(detail);
    }

    let profit_amount = current_amount - start_amount;
    let profit_pct = (profit_amount / start_amount) * dec!(100);

    let legs_arr: [LegDetail; 3] = legs.try_into().ok()?;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as u64;

    Some(ArbitrageOpportunity {
        path: path.clone(),
        legs: legs_arr,
        start_amount,
        end_amount: current_amount,
        profit_amount,
        profit_pct,
        detected_at_ms: now_ms,
    })
}

/// Evaluate a single leg: given an input amount in the "from" currency,
/// compute the output amount in the "to" currency after fees.
fn evaluate_leg(
    leg: &TradeLeg,
    ob: &OrderBook,
    input_amount: Decimal,
    taker_fee: Decimal,
    slippage_bps: Decimal,
) -> Option<LegDetail> {
    let fee_multiplier = Decimal::ONE - taker_fee;
    let slippage_multiplier = Decimal::ONE - slippage_bps / dec!(10000);

    match leg.side {
        OrderSide::Buy => {
            // We have quote currency, want to buy base currency.
            // Walk asks: we spend `input_amount` of quote to get base.
            let base_qty = buy_with_quote(ob, input_amount)?;
            let output = base_qty * fee_multiplier * slippage_multiplier;
            let effective_price = if base_qty > Decimal::ZERO {
                input_amount / base_qty
            } else {
                return None;
            };

            Some(LegDetail {
                symbol: leg.symbol.clone(),
                side: OrderSide::Buy,
                quantity: base_qty,
                effective_price,
                fee: base_qty - output,
                input_amount,
                output_amount: output,
            })
        }
        OrderSide::Sell => {
            // We have base currency, want to sell for quote currency.
            // Walk bids: sell `input_amount` base to get quote.
            let (eff_price, _) = ob.walk_bids(input_amount)?;
            let quote_proceeds = input_amount * eff_price;
            let output = quote_proceeds * fee_multiplier * slippage_multiplier;

            Some(LegDetail {
                symbol: leg.symbol.clone(),
                side: OrderSide::Sell,
                quantity: input_amount,
                effective_price: eff_price,
                fee: quote_proceeds - output,
                input_amount,
                output_amount: output,
            })
        }
    }
}

/// Given a quote amount to spend, walk asks and return the base quantity obtained.
fn buy_with_quote(ob: &OrderBook, quote_budget: Decimal) -> Option<Decimal> {
    let mut remaining_quote = quote_budget;
    let mut total_base = Decimal::ZERO;

    for (&price, &qty) in &ob.asks {
        if remaining_quote <= Decimal::ZERO {
            break;
        }
        let level_cost = price * qty;
        if level_cost <= remaining_quote {
            total_base += qty;
            remaining_quote -= level_cost;
        } else {
            let partial_qty = remaining_quote / price;
            total_base += partial_qty;
            remaining_quote = Decimal::ZERO;
        }
    }

    if total_base > Decimal::ZERO {
        Some(total_base)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::types::OrderBook;
    use std::collections::HashMap;

    fn make_ob(symbol: &str, bids: &[(Decimal, Decimal)], asks: &[(Decimal, Decimal)]) -> OrderBook {
        let mut ob = OrderBook::new(symbol.to_string());
        for &(p, q) in bids {
            ob.bids.insert(p, q);
        }
        for &(p, q) in asks {
            ob.asks.insert(p, q);
        }
        ob
    }

    #[test]
    fn test_profitable_triangle() {
        // Construct a scenario where triangular arb is profitable:
        // Start: 1000 USDT
        // Leg 1: Buy BTC with USDT (BTCUSDT sell side / asks)
        // Leg 2: Sell BTC for ETH (BTCETH bids)
        // Leg 3: Sell ETH for USDT (ETHUSDT bids)

        let mut books: HashMap<String, OrderBook> = HashMap::new();

        // BTCUSDT: ask 50000, so 1000 USDT buys 0.02 BTC
        books.insert(
            "BTCUSDT".into(),
            make_ob("BTCUSDT", &[(dec!(49990), dec!(10.0))], &[(dec!(50000), dec!(10.0))]),
        );
        // BTCETH: bid 20 ETH per BTC, so 0.02 BTC sells for 0.4 ETH
        books.insert(
            "BTCETH".into(),
            make_ob("BTCETH", &[(dec!(20), dec!(10.0))], &[(dec!(20.01), dec!(10.0))]),
        );
        // ETHUSDT: bid 2600, so 0.4 ETH sells for 1040 USDT
        books.insert(
            "ETHUSDT".into(),
            make_ob("ETHUSDT", &[(dec!(2600), dec!(100.0))], &[(dec!(2601), dec!(100.0))]),
        );

        let path = TrianglePath {
            start_asset: "USDT".into(),
            legs: [
                TradeLeg {
                    symbol: "BTCUSDT".into(),
                    side: OrderSide::Buy,
                    base_asset: "BTC".into(),
                    quote_asset: "USDT".into(),
                },
                TradeLeg {
                    symbol: "BTCETH".into(),
                    side: OrderSide::Sell,
                    base_asset: "BTC".into(),
                    quote_asset: "ETH".into(),
                },
                TradeLeg {
                    symbol: "ETHUSDT".into(),
                    side: OrderSide::Sell,
                    base_asset: "ETH".into(),
                    quote_asset: "USDT".into(),
                },
            ],
        };

        let opp = evaluate_triangle(
            &path,
            &|sym| books.get(sym).cloned(),
            dec!(1000),
            dec!(0.001),
            dec!(0),
        );

        assert!(opp.is_some());
        let opp = opp.unwrap();
        // With 0.1% fee per leg, starting 1000 USDT:
        // Leg1: 1000 / 50000 = 0.02 BTC, after fee: 0.02 * 0.999 = 0.01998 BTC
        // Leg2: 0.01998 * 20 = 0.3996 ETH, after fee: 0.3996 * 0.999 = 0.3992004 ETH
        // Leg3: 0.3992004 * 2600 = 1037.92..., after fee * 0.999 = ~1036.88
        // profit = 1036.88 - 1000 = ~36.88 USDT (~3.69%)
        assert!(opp.profit_amount > Decimal::ZERO);
        assert!(opp.profit_pct > dec!(3));
    }

    #[test]
    fn test_unprofitable_triangle() {
        let mut books: HashMap<String, OrderBook> = HashMap::new();

        // Tight spreads, no profit after fees
        books.insert(
            "BTCUSDT".into(),
            make_ob("BTCUSDT", &[(dec!(50000), dec!(10.0))], &[(dec!(50001), dec!(10.0))]),
        );
        books.insert(
            "BTCETH".into(),
            make_ob("BTCETH", &[(dec!(16.66), dec!(10.0))], &[(dec!(16.67), dec!(10.0))]),
        );
        books.insert(
            "ETHUSDT".into(),
            make_ob("ETHUSDT", &[(dec!(3000), dec!(100.0))], &[(dec!(3001), dec!(100.0))]),
        );

        let path = TrianglePath {
            start_asset: "USDT".into(),
            legs: [
                TradeLeg {
                    symbol: "BTCUSDT".into(),
                    side: OrderSide::Buy,
                    base_asset: "BTC".into(),
                    quote_asset: "USDT".into(),
                },
                TradeLeg {
                    symbol: "BTCETH".into(),
                    side: OrderSide::Sell,
                    base_asset: "BTC".into(),
                    quote_asset: "ETH".into(),
                },
                TradeLeg {
                    symbol: "ETHUSDT".into(),
                    side: OrderSide::Sell,
                    base_asset: "ETH".into(),
                    quote_asset: "USDT".into(),
                },
            ],
        };

        let opp = evaluate_triangle(
            &path,
            &|sym| books.get(sym).cloned(),
            dec!(1000),
            dec!(0.001),
            dec!(0),
        );

        // Should still return an opportunity, just not profitable
        assert!(opp.is_some());
        let opp = opp.unwrap();
        assert!(opp.profit_pct < dec!(0));
    }

    #[test]
    fn test_buy_with_quote() {
        let mut ob = OrderBook::new("TEST".into());
        ob.asks.insert(dec!(100), dec!(1.0));
        ob.asks.insert(dec!(101), dec!(2.0));

        // Spend 250: get 1.0 @ 100 = 100, then 1.485.. @ 101 = 150
        let base = buy_with_quote(&ob, dec!(250)).unwrap();
        // 1.0 + 150/101 = 1.0 + 1.4851.. ≈ 2.4851
        assert!(base > dec!(2.48) && base < dec!(2.49));
    }
}
