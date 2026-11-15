//! M-J: the user's number, date and time formats from `NSLocale.currentLocale` / `NSDateFormatter` (C#'s `CultureInfo.CurrentCulture`
//! reads on Windows). None until M-J: the shared code then keeps its own (a comma every three digits, ISO-like dates).

use std::time::SystemTime;

/// The grouping separator and sizes.
pub fn number_grouping() -> Option<(String, Vec<usize>)> {
    None
}

/// C#'s `ToString("g")`: the short date and time.
pub fn short_date_time(_at: SystemTime) -> Option<String> {
    None
}

/// The decimal separator.
pub fn decimal_separator() -> Option<String> {
    None
}

/// C#'s `ToString("T")`: the long time.
pub fn long_time(_at: SystemTime) -> Option<String> {
    None
}
