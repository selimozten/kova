use rust_decimal::Decimal;

/// Truncate a decimal to a given number of decimal places (round toward zero).
pub fn truncate(value: Decimal, decimals: u32) -> Decimal {
    value.round_dp_with_strategy(decimals, rust_decimal::RoundingStrategy::ToZero)
}

/// Round quantity to conform to exchange step size.
/// step_size is typically something like 0.001 or 0.00001.
pub fn round_to_step(value: Decimal, step_size: Decimal) -> Decimal {
    if step_size.is_zero() {
        return value;
    }
    (value / step_size).floor() * step_size
}

/// Round price to conform to exchange tick size.
pub fn round_to_tick(value: Decimal, tick_size: Decimal) -> Decimal {
    if tick_size.is_zero() {
        return value;
    }
    (value / tick_size).floor() * tick_size
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate(dec!(1.23456), 3), dec!(1.234));
        assert_eq!(truncate(dec!(1.23956), 2), dec!(1.23));
        assert_eq!(truncate(dec!(1.9), 0), dec!(1));
    }

    #[test]
    fn test_round_to_step() {
        assert_eq!(round_to_step(dec!(1.23456), dec!(0.001)), dec!(1.234));
        assert_eq!(round_to_step(dec!(0.15), dec!(0.01)), dec!(0.15));
        assert_eq!(round_to_step(dec!(123.456), dec!(1)), dec!(123));
    }

    #[test]
    fn test_round_to_tick() {
        assert_eq!(round_to_tick(dec!(100.123), dec!(0.01)), dec!(100.12));
        assert_eq!(round_to_tick(dec!(99.999), dec!(0.1)), dec!(99.9));
    }
}
