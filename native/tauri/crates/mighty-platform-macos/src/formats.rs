//! M-J: the user's number, date and time formats from CoreFoundation's current locale (`CFLocaleCopyCurrent`, the region and the
//! changes the user made in System Settings), as C#'s `CultureInfo.CurrentCulture` reads them on Windows: the group separator and
//! sizes, the decimal separator, `ToString("g")` (the short date and time) and `ToString("T")` (the time with seconds), in the local
//! time zone. None when CoreFoundation does not say: the shared code then keeps its own.

use std::ffi::c_void;
use std::time::SystemTime;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSNumber, NSString};

type Ref = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFNumberFormatterGroupingSeparator: Ref;
    static kCFNumberFormatterDecimalSeparator: Ref;
    static kCFNumberFormatterGroupingSize: Ref;
    static kCFNumberFormatterSecondaryGroupingSize: Ref;
    static kCFNumberFormatterUseGroupingSeparator: Ref;
    fn CFLocaleCopyCurrent() -> Ref;
    fn CFLocaleCreate(allocator: Ref, identifier: Ref) -> Ref;
    fn CFNumberFormatterCreate(allocator: Ref, locale: Ref, style: isize) -> Ref;
    fn CFNumberFormatterCopyProperty(formatter: Ref, key: Ref) -> Ref;
    fn CFDateFormatterCreate(allocator: Ref, locale: Ref, date_style: isize, time_style: isize) -> Ref;
    fn CFDateFormatterCreateStringWithAbsoluteTime(allocator: Ref, formatter: Ref, at: f64) -> Ref;
}

/// `kCFNumberFormatterDecimalStyle`.
const DECIMAL_STYLE: isize = 1;
/// `kCFDateFormatterNoStyle`, `ShortStyle`, `MediumStyle`.
const NO_STYLE: isize = 0;
const SHORT_STYLE: isize = 1;
const MEDIUM_STYLE: isize = 2;
/// Seconds from 1970 to CoreFoundation's reference date (2001-01-01 UTC).
const REFERENCE_DATE: f64 = 978_307_200.0;

/// A +1 CoreFoundation object (toll-free bridged with Foundation's), released when dropped; None for NULL.
fn owned(object: Ref) -> Option<Retained<AnyObject>> {
    // SAFETY: every caller passes a Create/Copy result: a +1 reference to a CF object, released by the Retained.
    unsafe { Retained::from_raw(object as *mut AnyObject) }
}

fn ptr(object: &AnyObject) -> Ref {
    object as *const AnyObject as Ref
}

fn text(object: Option<Retained<AnyObject>>) -> Option<String> {
    Some(object?.downcast_ref::<NSString>()?.to_string())
}

fn integer(object: Option<Retained<AnyObject>>) -> Option<i64> {
    Some(object?.downcast_ref::<NSNumber>()?.as_i64())
}

fn on(object: Option<Retained<AnyObject>>) -> Option<bool> {
    Some(object?.downcast_ref::<NSNumber>()?.as_bool())
}

/// The user's current locale, or (tests) the one named.
fn locale(identifier: Option<&str>) -> Option<Retained<AnyObject>> {
    match identifier {
        // SAFETY: plain CF calls; the identifier is a live NSString (toll-free bridged with CFString) for the call.
        Some(name) => owned(unsafe { CFLocaleCreate(std::ptr::null(), ptr(&NSString::from_str(name))) }),
        None => owned(unsafe { CFLocaleCopyCurrent() }),
    }
}

fn number_formatter(identifier: Option<&str>) -> Option<Retained<AnyObject>> {
    let locale = locale(identifier)?;
    // SAFETY: a live locale; the result is +1.
    owned(unsafe { CFNumberFormatterCreate(std::ptr::null(), ptr(&locale), DECIMAL_STYLE) })
}

fn property(formatter: &AnyObject, key: Ref) -> Option<Retained<AnyObject>> {
    // SAFETY: a live formatter and one of CoreFoundation's own keys; Copy returns +1 (or NULL).
    owned(unsafe { CFNumberFormatterCopyProperty(ptr(formatter), key) })
}

/// .NET's `NumberGroupSizes` from ICU's primary and secondary sizes: `[3]` repeats three, India's `[3, 2]` three then twos, `[0]`
/// for no grouping.
fn group_sizes(primary: i64, secondary: i64, grouping: bool) -> Vec<usize> {
    if !grouping || primary <= 0 {
        return vec![0];
    }
    let primary = primary as usize;
    if secondary > 0 && secondary as usize != primary { vec![primary, secondary as usize] } else { vec![primary] }
}

fn grouping_in(identifier: Option<&str>) -> Option<(String, Vec<usize>)> {
    let formatter = number_formatter(identifier)?;
    // SAFETY: CoreFoundation's own constant keys.
    let (separator, primary, secondary, grouping) = unsafe {
        (
            text(property(&formatter, kCFNumberFormatterGroupingSeparator))?,
            integer(property(&formatter, kCFNumberFormatterGroupingSize)).unwrap_or(3),
            integer(property(&formatter, kCFNumberFormatterSecondaryGroupingSize)).unwrap_or(0),
            on(property(&formatter, kCFNumberFormatterUseGroupingSeparator)).unwrap_or(true),
        )
    };
    Some((separator, group_sizes(primary, secondary, grouping)))
}

fn decimal_in(identifier: Option<&str>) -> Option<String> {
    let formatter = number_formatter(identifier)?;
    // SAFETY: CoreFoundation's own constant key.
    text(property(&formatter, unsafe { kCFNumberFormatterDecimalSeparator })).filter(|s| !s.is_empty())
}

fn date_in(identifier: Option<&str>, at: SystemTime, date_style: isize, time_style: isize) -> Option<String> {
    let seconds = at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs_f64();
    let locale = locale(identifier)?;
    // SAFETY: a live locale; the formatter is +1, and so is its string.
    let formatter = owned(unsafe { CFDateFormatterCreate(std::ptr::null(), ptr(&locale), date_style, time_style) })?;
    text(owned(unsafe { CFDateFormatterCreateStringWithAbsoluteTime(std::ptr::null(), ptr(&formatter), seconds - REFERENCE_DATE) }))
        .filter(|s| !s.is_empty())
}

/// The grouping separator and sizes (C# `NumberFormatInfo.NumberGroupSeparator`, `NumberGroupSizes`).
pub fn number_grouping() -> Option<(String, Vec<usize>)> {
    grouping_in(None)
}

/// C#'s `ToString("g")`: the short date and the short time.
pub fn short_date_time(at: SystemTime) -> Option<String> {
    date_in(None, at, SHORT_STYLE, SHORT_STYLE)
}

/// The decimal separator.
pub fn decimal_separator() -> Option<String> {
    decimal_in(None)
}

/// C#'s `ToString("T")`: the time with its seconds (the medium time style; the long one adds the zone).
pub fn long_time(at: SystemTime) -> Option<String> {
    date_in(None, at, NO_STYLE, MEDIUM_STYLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes read as .NET's.
    #[test]
    fn group_sizes_read_as_dotnets() {
        assert_eq!(group_sizes(3, 0, true), [3]);
        assert_eq!(group_sizes(3, 3, true), [3]);
        assert_eq!(group_sizes(3, 2, true), [3, 2]);
        assert_eq!(group_sizes(3, 0, false), [0]);
    }

    /// Named regions give their own forms: the US a comma every three digits and a point, Germany the other way round, India
    /// three then twos; the user's own locale answers too.
    #[test]
    fn each_region_gives_its_own_number_forms() {
        assert_eq!(grouping_in(Some("en_US")), Some((",".to_string(), vec![3])));
        assert_eq!(decimal_in(Some("en_US")).as_deref(), Some("."));
        assert_eq!(grouping_in(Some("de_DE")), Some((".".to_string(), vec![3])));
        assert_eq!(decimal_in(Some("de_DE")).as_deref(), Some(","));
        assert_eq!(grouping_in(Some("hi_IN")).map(|(_, sizes)| sizes), Some(vec![3, 2]));
        assert!(number_grouping().is_some() && decimal_separator().is_some());
    }

    /// The short date and time, and the time with seconds, in the region's form (the hour depends on the runner's zone, so only the
    /// shape and the minutes and seconds are compared).
    #[test]
    fn dates_and_times_are_the_regions() {
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_794_798_245); // 2026-11-16 03:04:05 UTC
        let short = date_in(Some("en_US"), at, SHORT_STYLE, SHORT_STYLE).unwrap();
        assert!(short.contains("/26") && short.contains(":04"), "{short}");
        let time = date_in(Some("en_US"), at, NO_STYLE, MEDIUM_STYLE).unwrap();
        assert!(time.contains(":04:05") && !time.contains("/"), "{time}");
        let german = date_in(Some("de_DE"), at, SHORT_STYLE, SHORT_STYLE).unwrap();
        assert!(german.contains(".11.26") || german.contains(".11.2026"), "{german}");
        assert!(short_date_time(at).is_some() && long_time(at).is_some());
    }
}
