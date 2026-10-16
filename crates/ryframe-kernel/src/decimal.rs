use rust_decimal::Decimal;

/// 完整 SQL 数值域须落在原生 Decimal 容量内，校验不会舍入或改变值。
pub fn decimal_fits_precision(value: Decimal, precision: u32, scale: u32) -> bool {
    if !(1..=28).contains(&precision) || scale > precision || value.scale() > scale {
        return false;
    }
    let integer_digits = if value.is_zero() {
        0
    } else {
        (value.mantissa().unsigned_abs().ilog10() + 1).saturating_sub(value.scale())
    };
    integer_digits <= precision - scale
}
