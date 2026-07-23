use std::collections::HashMap;

use ccc_diag::WarningLevel;

/// A warning name accepted by the driver. Keeping this inventory here makes
/// typoed warning options fail closed instead of silently changing a build's
/// requested diagnostic policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WarningCategorySpec {
    name: &'static str,
    default_enabled: bool,
}

const WARNING_CATEGORIES: &[WarningCategorySpec] = &[
    WarningCategorySpec {
        name: "cpp",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "degraded-hardening",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "deprecated-declarations",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "inline",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "macro-redefined",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "missing-field-initializers",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "pedantic",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "strict-prototypes",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "system-headers",
        default_enabled: false,
    },
    WarningCategorySpec {
        name: "trigraphs",
        default_enabled: true,
    },
    WarningCategorySpec {
        name: "unknown-pragmas",
        default_enabled: true,
    },
];

/// Positive aggregate selectors accepted for build-system compatibility.
/// CCC's currently emitted warnings are already enabled by default, so these
/// spellings do not broaden the warning set. Their negative and per-category
/// promotion forms are deliberately not accepted because CCC does not yet
/// model the membership of those external compiler groups.
const POSITIVE_WARNING_GROUPS: &[&str] = &["-W", "-Wall", "-Wextra"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CategoryAction<'a> {
    Enable(&'a str),
    Disable(&'a str),
    Promote(&'a str),
    Demote(&'a str),
}

impl CategoryAction<'_> {
    fn category(self) -> &'static str {
        let name = match self {
            Self::Enable(name) | Self::Disable(name) | Self::Promote(name) | Self::Demote(name) => {
                name
            }
        };
        warning_spec(name)
            .expect("validated warning option has a registry entry")
            .name
    }
}

fn category_action(option: &str) -> Option<CategoryAction<'_>> {
    if let Some(category) = option.strip_prefix("-Werror=") {
        Some(CategoryAction::Promote(category))
    } else if let Some(category) = option.strip_prefix("-Wno-error=") {
        Some(CategoryAction::Demote(category))
    } else if let Some(category) = option.strip_prefix("-Wno-") {
        Some(CategoryAction::Disable(category))
    } else {
        option
            .strip_prefix("-W")
            .filter(|category| !category.is_empty())
            .map(CategoryAction::Enable)
    }
}

fn warning_spec(name: &str) -> Option<WarningCategorySpec> {
    WARNING_CATEGORIES
        .iter()
        .copied()
        .find(|spec| spec.name == name)
}

pub(crate) fn validate_warning_option(option: &str) -> Result<(), String> {
    if POSITIVE_WARNING_GROUPS.contains(&option) {
        return Ok(());
    }
    let Some(action) = category_action(option) else {
        return Err(format!("ccc: unknown warning option `{option}`"));
    };
    let category = match action {
        CategoryAction::Enable(category)
        | CategoryAction::Disable(category)
        | CategoryAction::Promote(category)
        | CategoryAction::Demote(category) => category,
    };
    if category.is_empty() || warning_spec(category).is_none() {
        return Err(format!("ccc: unknown warning option `{option}`"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Promotion {
    #[default]
    Inherit,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CategoryState {
    enabled: Option<bool>,
    promotion: Promotion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WarningDisposition {
    Suppressed,
    Warning,
    Error,
}

/// Resolves source-ordered category options into one effective policy. Global
/// `-Werror` remains less specific than a per-category promotion or demotion,
/// matching GCC/Clang warning-option precedence.
#[derive(Clone, Debug)]
pub(crate) struct WarningPolicy {
    suppress_all: bool,
    warnings_as_errors: bool,
    states: HashMap<&'static str, CategoryState>,
}

impl WarningPolicy {
    pub(crate) fn new(suppress_all: bool, warnings_as_errors: bool, options: &[String]) -> Self {
        let mut policy = Self {
            suppress_all,
            warnings_as_errors,
            states: HashMap::new(),
        };
        for option in options {
            if POSITIVE_WARNING_GROUPS.contains(&option.as_str()) {
                continue;
            }
            let action =
                category_action(option).expect("driver stores only validated warning options");
            let category = action.category();
            let state = policy.states.entry(category).or_default();
            match action {
                CategoryAction::Enable(_) => state.enabled = Some(true),
                CategoryAction::Disable(_) => {
                    state.enabled = Some(false);
                    // `-Werror=name` implies `-Wname`; disabling that implied
                    // category cancels the promotion if a later `-Wname`
                    // enables it again. An explicit `-Wno-error=name` remains
                    // a category-specific override of global `-Werror`.
                    if state.promotion == Promotion::Error {
                        state.promotion = Promotion::Inherit;
                    }
                }
                CategoryAction::Promote(_) => {
                    state.enabled = Some(true);
                    state.promotion = Promotion::Error;
                }
                CategoryAction::Demote(_) => state.promotion = Promotion::Warning,
            }
        }
        policy
    }

    pub(crate) fn level(&self, category: &str) -> WarningLevel {
        if self.suppress_all {
            return WarningLevel::Ignored;
        }
        let spec = warning_spec(category).expect("queried warning category is registered");
        let state = self.states.get(spec.name).copied().unwrap_or_default();
        if !state.enabled.unwrap_or(spec.default_enabled) {
            return WarningLevel::Ignored;
        }
        match state.promotion {
            Promotion::Inherit => WarningLevel::Default,
            Promotion::Warning => WarningLevel::Warning,
            Promotion::Error => WarningLevel::Error,
        }
    }

    pub(crate) fn enabled(&self, category: &str) -> bool {
        self.level(category) != WarningLevel::Ignored
    }

    pub(crate) fn disposition(&self, category: &str) -> WarningDisposition {
        match self.level(category) {
            WarningLevel::Ignored => WarningDisposition::Suppressed,
            WarningLevel::Error => WarningDisposition::Error,
            WarningLevel::Warning => WarningDisposition::Warning,
            WarningLevel::Default if self.warnings_as_errors => WarningDisposition::Error,
            WarningLevel::Default => WarningDisposition::Warning,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(global_error: bool, options: &[&str]) -> WarningPolicy {
        WarningPolicy::new(
            false,
            global_error,
            &options
                .iter()
                .map(|option| (*option).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn registry_rejects_unknown_categories_and_unsupported_group_directions() {
        for option in [
            "-Wtypo",
            "-Wno-typo",
            "-Werror=typo",
            "-Wno-error=typo",
            "-Werror=",
            "-Wno-error=",
            "-Wno-all",
            "-Werror=all",
        ] {
            assert!(validate_warning_option(option).is_err(), "{option}");
        }
        for option in [
            "-W",
            "-Wall",
            "-Wextra",
            "-Wdegraded-hardening",
            "-Wno-degraded-hardening",
            "-Werror=degraded-hardening",
            "-Wno-error=degraded-hardening",
        ] {
            validate_warning_option(option).unwrap();
        }
    }

    #[test]
    fn category_enable_disable_and_promotion_follow_specificity_and_order() {
        use WarningDisposition::{Error, Suppressed, Warning};

        let cases: &[(bool, &[&str], WarningDisposition)] = &[
            (false, &[], Warning),
            (true, &[], Error),
            (false, &["-Wno-degraded-hardening"], Suppressed),
            (
                false,
                &["-Wno-degraded-hardening", "-Wdegraded-hardening"],
                Warning,
            ),
            (
                false,
                &["-Wdegraded-hardening", "-Wno-degraded-hardening"],
                Suppressed,
            ),
            (false, &["-Werror=degraded-hardening"], Error),
            (true, &["-Wno-error=degraded-hardening"], Warning),
            (
                false,
                &[
                    "-Werror=degraded-hardening",
                    "-Wno-error=degraded-hardening",
                ],
                Warning,
            ),
            (
                false,
                &[
                    "-Wno-error=degraded-hardening",
                    "-Werror=degraded-hardening",
                ],
                Error,
            ),
            (
                false,
                &[
                    "-Werror=degraded-hardening",
                    "-Wno-degraded-hardening",
                    "-Wdegraded-hardening",
                ],
                Warning,
            ),
            (
                true,
                &[
                    "-Wno-error=degraded-hardening",
                    "-Wno-degraded-hardening",
                    "-Wdegraded-hardening",
                ],
                Warning,
            ),
        ];
        for (global_error, options, expected) in cases {
            assert_eq!(
                policy(*global_error, options).disposition("degraded-hardening"),
                *expected,
                "global_error={global_error}, options={options:?}"
            );
        }
    }

    #[test]
    fn no_error_does_not_enable_a_disabled_category() {
        let policy = policy(
            true,
            &["-Wno-degraded-hardening", "-Wno-error=degraded-hardening"],
        );
        assert_eq!(
            policy.disposition("degraded-hardening"),
            WarningDisposition::Suppressed
        );
    }

    #[test]
    fn system_header_warnings_are_opt_in_and_last_switch_wins() {
        assert!(!policy(false, &[]).enabled("system-headers"));
        assert!(
            policy(
                false,
                &[
                    "-Wsystem-headers",
                    "-Wno-system-headers",
                    "-Wsystem-headers"
                ]
            )
            .enabled("system-headers")
        );
    }
}
