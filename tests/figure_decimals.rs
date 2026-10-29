//! Phase 2499 — a currency figure can set its fraction digits.
//!
//! Every figure case carries `decimals`, `Currency` included, on both format
//! vocabularies. Absent, a currency renders as before (`CODE x.xx`); present, it
//! is the digit count of this host's locale-free form and of its invariant
//! `Format` fallback, which a located figure routes to.

use fuaran_rs::render::bindings::{format_locale_value, format_number};
use fuaran_rs::wire::{CellFormat, Format, LocaleSource};

fn gbp(decimals: Option<i64>, locale: Option<LocaleSource>) -> CellFormat {
    CellFormat::Currency {
        code: "GBP".to_string(),
        decimals,
        locale,
    }
}

#[test]
fn a_present_decimals_sets_the_locale_free_currency_digits() {
    assert_eq!(format_number(&gbp(None, None), 412380.0), "GBP 412380.00");
    assert_eq!(format_number(&gbp(Some(0), None), 412380.4), "GBP 412380");
    assert_eq!(format_number(&gbp(Some(3), None), 1.5), "GBP 1.500");
}

#[test]
fn a_located_currency_hands_its_decimals_to_the_format_fallback() {
    let en_gb = LocaleSource::Explicit {
        tag: "en-GB".to_string(),
    };
    let located = format_number(&gbp(Some(0), Some(en_gb.clone())), 412380.0);
    let routed = format_locale_value(
        &en_gb,
        &Format::Currency {
            iso_code: "GBP".to_string(),
            decimals: Some(0),
        },
        412380.0,
    );
    assert_eq!(located, routed);
    assert_eq!(located, "GBP 412380");
}
