use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use ccc_target::{CapabilityKind, EffectiveCompilationConfig, OptimizationLevel, RelocationModel};

pub(crate) fn additional_predefined_macros(
    config: &EffectiveCompilationConfig,
) -> BTreeMap<String, String> {
    let mut macros = BTreeMap::new();

    for (name, replacement) in [
        ("__STDC_HOSTED__", "1"),
        ("__CCC__", "1"),
        ("__CCC_MAJOR__", "0"),
        ("__CCC_MINOR__", "1"),
        ("__CCC_PATCHLEVEL__", "0"),
    ] {
        macros.insert(name.to_owned(), replacement.to_owned());
    }

    match config.optimization {
        OptimizationLevel::O0 => {
            macros.insert("__NO_INLINE__".to_owned(), "1".to_owned());
        }
        OptimizationLevel::O1 | OptimizationLevel::O2 | OptimizationLevel::O3 => {
            macros.insert("__OPTIMIZE__".to_owned(), "1".to_owned());
        }
        OptimizationLevel::Size | OptimizationLevel::SizeMin => {
            macros.insert("__OPTIMIZE__".to_owned(), "1".to_owned());
            macros.insert("__OPTIMIZE_SIZE__".to_owned(), "1".to_owned());
        }
    }

    if matches!(
        config.relocation_model,
        RelocationModel::Pic | RelocationModel::Pie
    ) {
        for name in ["__PIC__", "__pic__"] {
            macros.insert(name.to_owned(), "2".to_owned());
        }
    }
    if config.relocation_model == RelocationModel::Pie {
        for name in ["__PIE__", "__pie__"] {
            macros.insert(name.to_owned(), "2".to_owned());
        }
    }

    for (name, capability) in [
        ("__STDC_NO_ATOMICS__", "c11-atomics"),
        ("__STDC_NO_COMPLEX__", "c11-complex"),
        ("__STDC_NO_THREADS__", "c11-threads"),
        ("__STDC_NO_VLA__", "c11-vla"),
    ] {
        if !config
            .capabilities
            .is_available(CapabilityKind::Feature, capability)
        {
            macros.insert(name.to_owned(), "1".to_owned());
        }
    }
    macros
}

pub(crate) fn feature_predicates(config: &EffectiveCompilationConfig) -> BTreeMap<String, bool> {
    config
        .capabilities
        .iter()
        .map(|(key, entry)| {
            let family = match key.kind {
                CapabilityKind::Attribute => "attribute",
                CapabilityKind::Builtin => "builtin",
                CapabilityKind::Extension => "extension",
                CapabilityKind::Feature => "feature",
                CapabilityKind::Pragma => "pragma",
            };
            (format!("{family}:{}", key.name), entry.state.is_available())
        })
        .collect()
}

pub(crate) fn translation_date_and_time() -> (String, String) {
    let seconds = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        })
        .unwrap_or(0);
    format_timestamp(seconds)
}

fn format_timestamp(seconds: i64) -> (String, String) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    let second = seconds_in_day % 60;
    let month_name = MONTHS[usize::try_from(month - 1).expect("month is in range")];
    (
        format!("\"{month_name} {day:2} {year:04}\""),
        format!("\"{hour:02}:{minute:02}:{second:02}\""),
    )
}

// Converts days since 1970-01-01 to a proleptic Gregorian date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_piece = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_piece + 2) / 5 + 1;
    let month = month_piece + if month_piece < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccc_target::LanguageMode;

    #[test]
    fn derives_standard_and_target_macros_from_the_configuration() {
        let config = EffectiveCompilationConfig::default();
        let mut macros = config.frontend_predefined_macros();
        macros.extend(additional_predefined_macros(&config));
        assert_eq!(
            macros.get("__STDC_VERSION__").map(String::as_str),
            Some("201112L")
        );
        assert_eq!(macros.get("__GNUC__").map(String::as_str), Some("4"));
        assert_eq!(
            macros.get("__USER_LABEL_PREFIX__").map(String::as_str),
            Some("")
        );
        assert_eq!(
            macros.get("__SIZEOF_POINTER__").map(String::as_str),
            Some("8")
        );
        for unsupported in [
            "__STDC_NO_ATOMICS__",
            "__STDC_NO_COMPLEX__",
            "__STDC_NO_THREADS__",
            "__STDC_NO_VLA__",
        ] {
            assert_eq!(
                macros.get(unsupported).map(String::as_str),
                Some("1"),
                "missing denial macro {unsupported}"
            );
        }
        for (name, expected) in [
            ("__CCC__", "1"),
            ("__CCC_MAJOR__", "0"),
            ("__CCC_MINOR__", "1"),
            ("__CCC_PATCHLEVEL__", "0"),
            ("__GNUC__", "4"),
            ("__GNUC_MINOR__", "2"),
            ("__GNUC_PATCHLEVEL__", "1"),
            ("__PIC__", "2"),
            ("__pic__", "2"),
            ("__PIE__", "2"),
            ("__pie__", "2"),
        ] {
            assert_eq!(macros.get(name).map(String::as_str), Some(expected));
        }
    }

    #[test]
    fn static_relocation_model_omits_position_independent_macros() {
        let config = EffectiveCompilationConfig {
            relocation_model: RelocationModel::Static,
            ..EffectiveCompilationConfig::default()
        };

        let macros = additional_predefined_macros(&config);
        for name in ["__PIC__", "__pic__", "__PIE__", "__pie__"] {
            assert!(!macros.contains_key(name), "unexpected macro {name}");
        }
    }

    #[test]
    fn pic_relocation_model_does_not_advertise_a_pie_compilation() {
        let config = EffectiveCompilationConfig {
            relocation_model: RelocationModel::Pic,
            ..EffectiveCompilationConfig::default()
        };

        let macros = additional_predefined_macros(&config);
        assert_eq!(macros.get("__PIC__").map(String::as_str), Some("2"));
        assert_eq!(macros.get("__pic__").map(String::as_str), Some("2"));
        assert!(!macros.contains_key("__PIE__"));
        assert!(!macros.contains_key("__pie__"));
    }

    #[test]
    fn optimization_macros_follow_the_selected_profile() {
        for (optimization, optimize, size, no_inline) in [
            (OptimizationLevel::O0, false, false, true),
            (OptimizationLevel::O1, true, false, false),
            (OptimizationLevel::O2, true, false, false),
            (OptimizationLevel::O3, true, false, false),
            (OptimizationLevel::Size, true, true, false),
            (OptimizationLevel::SizeMin, true, true, false),
        ] {
            let config = EffectiveCompilationConfig {
                optimization,
                ..EffectiveCompilationConfig::default()
            };
            let macros = additional_predefined_macros(&config);
            assert_eq!(macros.contains_key("__OPTIMIZE__"), optimize);
            assert_eq!(macros.contains_key("__OPTIMIZE_SIZE__"), size);
            assert_eq!(macros.contains_key("__NO_INLINE__"), no_inline);
        }
    }

    #[test]
    fn strict_language_mode_reports_strict_ansi_without_changing_identity() {
        let config = EffectiveCompilationConfig::default().with_language_mode(LanguageMode::C11);
        let mut macros = config.frontend_predefined_macros();
        macros.extend(additional_predefined_macros(&config));

        assert_eq!(macros.get("__STRICT_ANSI__").map(String::as_str), Some("1"));
        assert_eq!(
            macros.get("__STDC_VERSION__").map(String::as_str),
            Some("201112L")
        );
        assert_eq!(macros.get("__GNUC__").map(String::as_str), Some("4"));
    }

    #[test]
    fn denial_macros_follow_the_capability_registry() {
        let mut config = EffectiveCompilationConfig::default();
        config.capabilities.insert(
            CapabilityKind::Feature,
            "c11-vla",
            ccc_target::CapabilityState::Implemented,
        );
        config.capabilities.insert(
            CapabilityKind::Feature,
            "c11-atomics",
            ccc_target::CapabilityState::Unsupported,
        );

        let macros = additional_predefined_macros(&config);
        assert!(!macros.contains_key("__STDC_NO_VLA__"));
        assert_eq!(
            macros.get("__STDC_NO_ATOMICS__").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn formats_reproducible_translation_timestamps() {
        assert_eq!(
            format_timestamp(0),
            ("\"Jan  1 1970\"".to_owned(), "\"00:00:00\"".to_owned())
        );
        assert_eq!(
            format_timestamp(951_827_696),
            ("\"Feb 29 2000\"".to_owned(), "\"12:34:56\"".to_owned())
        );
    }
}
