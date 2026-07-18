use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use ccc_target::GnuCompatibilityProfile;

const RESOURCE_FORMAT_VERSION: u64 = 2;
const GNU_PROFILE_NAME: &str = "gcc-4.2.1";
const GNU_PROFILE_VERSION: &str = "4.2.1";
const GNU_PROFILE_SCOPE: &str = "code-generation";
const GNU_PROFILE_SELECTION_GATE: &str = "__GNUC_PREREQ(4, 2)";
const GNU_PROFILE_CAPABILITIES: &[&str] = &[
    "computed-includes",
    "function-like-macros",
    "gcc-diagnostic-pragma",
    "gcc-system-header-pragma",
    "gnu-comma-elision",
    "gnu-alternative-keywords",
    "gnu-attribute-specifiers",
    "gnu-declaration-asm-labels",
    "gnu-extension-marker",
    "gnu-named-variadic-macros",
    "gnu-restrict-qualifiers",
    "gnu-typeof",
    "include-next",
    "line-control",
    "object-like-macros",
    "pragma-operator",
    "token-pasting",
    "token-stringification",
    "variadic-macros",
    "warning-directive",
];
const GNU_PROFILE_DECLINED_CAPABILITIES: &[&str] = &["va-opt"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResourceDirectory {
    root: PathBuf,
    include: PathBuf,
    hosted_header_profile: GnuCompatibilityProfile,
}

impl ResourceDirectory {
    pub(crate) fn discover(explicit: Option<&Path>) -> Result<Self, String> {
        if let Some(root) = explicit {
            return Self::load(root.to_path_buf());
        }

        if let Some(root) = std::env::var_os("CCC_RESOURCE_DIR") {
            return Self::load(PathBuf::from(root));
        }

        let development = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resource-dir");
        if development.join("manifest.toml").is_file() {
            return Self::load(development);
        }

        let executable = std::env::current_exe()
            .map_err(|error| format!("cannot locate the compiler executable: {error}"))?;
        let installed = executable
            .parent()
            .and_then(Path::parent)
            .map(|prefix| prefix.join("lib/ccc/resource-dir"))
            .ok_or_else(|| "cannot derive the installed resource directory".to_owned())?;
        Self::load(installed)
    }

    fn load(root: PathBuf) -> Result<Self, String> {
        let manifest_path = root.join("manifest.toml");
        let source = fs::read_to_string(&manifest_path).map_err(|error| {
            format!(
                "cannot read resource manifest {}: {error}",
                manifest_path.display()
            )
        })?;
        let manifest = ResourceManifest::parse(&source).map_err(|error| {
            format!(
                "invalid resource manifest {}: {error}",
                manifest_path.display()
            )
        })?;
        manifest.validate(&root, &manifest_path)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn include(&self) -> &Path {
        &self.include
    }

    pub(crate) fn hosted_header_profile(&self) -> &GnuCompatibilityProfile {
        &self.hosted_header_profile
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResourceManifest {
    format_version: u64,
    compiler_version: String,
    headers: HeaderManifest,
    hosted_header_profile: HostedHeaderProfileManifest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HeaderManifest {
    directory: String,
    compiler_owned: Vec<String>,
    target_derived: Vec<String>,
    hosted_wrappers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HostedHeaderProfileManifest {
    name: String,
    version: String,
    scope: String,
    selection_gate: String,
    rationale: String,
    capabilities: Vec<String>,
    declined_capabilities: Vec<String>,
}

impl ResourceManifest {
    fn parse(source: &str) -> Result<Self, String> {
        let mut document = ManifestDocument::parse(source)?;
        let manifest = Self {
            format_version: document.integer("format_version")?,
            compiler_version: document.string("compiler_version")?,
            headers: HeaderManifest {
                directory: document.string("headers.directory")?,
                compiler_owned: document.strings("headers.compiler_owned")?,
                target_derived: document.strings("headers.target_derived")?,
                hosted_wrappers: document.strings("headers.hosted_wrappers")?,
            },
            hosted_header_profile: HostedHeaderProfileManifest {
                name: document.string("hosted_header_profile.name")?,
                version: document.string("hosted_header_profile.version")?,
                scope: document.string("hosted_header_profile.scope")?,
                selection_gate: document.string("hosted_header_profile.selection_gate")?,
                rationale: document.string("hosted_header_profile.rationale")?,
                capabilities: document.strings("hosted_header_profile.capabilities")?,
                declined_capabilities: document
                    .strings("hosted_header_profile.declined_capabilities")?,
            },
        };
        document.finish()?;
        Ok(manifest)
    }

    fn validate(self, root: &Path, manifest_path: &Path) -> Result<ResourceDirectory, String> {
        if self.format_version != RESOURCE_FORMAT_VERSION {
            return Err(format!(
                "resource manifest {} uses format version {}, expected {}",
                manifest_path.display(),
                self.format_version,
                RESOURCE_FORMAT_VERSION
            ));
        }
        if self.compiler_version != env!("CARGO_PKG_VERSION") {
            return Err(format!(
                "resource manifest {} targets compiler version {}, expected {}",
                manifest_path.display(),
                self.compiler_version,
                env!("CARGO_PKG_VERSION")
            ));
        }
        let hosted_header_profile = self.hosted_header_profile.validate(manifest_path)?;

        let directory = checked_relative_path(&self.headers.directory, "headers.directory")?;
        let include = root.join(directory);
        if !include.is_dir() {
            return Err(format!(
                "resource include directory {} is missing",
                include.display()
            ));
        }

        let mut classified = BTreeMap::new();
        for (class, headers) in [
            ("headers.compiler_owned", &self.headers.compiler_owned),
            ("headers.target_derived", &self.headers.target_derived),
            ("headers.hosted_wrappers", &self.headers.hosted_wrappers),
        ] {
            for header in headers {
                let path = checked_relative_path(header, &format!("{class} entry"))?;
                let normalized = slash_path(&path);
                if let Some(previous) = classified.insert(normalized.clone(), class) {
                    if previous == class {
                        return Err(format!(
                            "resource manifest {} lists header {normalized:?} more than once in {class}",
                            manifest_path.display()
                        ));
                    }
                    return Err(format!(
                        "resource manifest {} classifies header {normalized:?} in both {previous} and {class}",
                        manifest_path.display()
                    ));
                }
                let header_path = include.join(path);
                if !header_path.is_file() {
                    return Err(format!(
                        "resource manifest {} lists missing header {} in {class}",
                        manifest_path.display(),
                        header_path.display()
                    ));
                }
            }
        }

        let discovered = header_inventory(&include)?;
        let listed = classified.keys().cloned().collect::<BTreeSet<_>>();
        let missing_from_manifest = discovered.difference(&listed).cloned().collect::<Vec<_>>();
        let absent_from_disk = listed.difference(&discovered).cloned().collect::<Vec<_>>();
        if !missing_from_manifest.is_empty() || !absent_from_disk.is_empty() {
            return Err(format!(
                "resource header inventory mismatch in {}: unlisted [{}], absent [{}]",
                manifest_path.display(),
                missing_from_manifest.join(", "),
                absent_from_disk.join(", ")
            ));
        }

        Ok(ResourceDirectory {
            root: root.to_path_buf(),
            include,
            hosted_header_profile,
        })
    }
}

impl HostedHeaderProfileManifest {
    fn validate(&self, manifest_path: &Path) -> Result<GnuCompatibilityProfile, String> {
        for (field, actual, expected) in [
            ("name", self.name.as_str(), GNU_PROFILE_NAME),
            ("version", self.version.as_str(), GNU_PROFILE_VERSION),
            ("scope", self.scope.as_str(), GNU_PROFILE_SCOPE),
            (
                "selection_gate",
                self.selection_gate.as_str(),
                GNU_PROFILE_SELECTION_GATE,
            ),
        ] {
            if actual != expected {
                return Err(format!(
                    "resource manifest {} has hosted_header_profile.{field} {actual:?}, expected {expected:?}",
                    manifest_path.display()
                ));
            }
        }
        if self.rationale.trim().is_empty() {
            return Err(format!(
                "resource manifest {} has an empty hosted-header profile rationale",
                manifest_path.display()
            ));
        }
        validate_profile_set(
            manifest_path,
            "capabilities",
            &self.capabilities,
            GNU_PROFILE_CAPABILITIES,
        )?;
        validate_profile_set(
            manifest_path,
            "declined_capabilities",
            &self.declined_capabilities,
            GNU_PROFILE_DECLINED_CAPABILITIES,
        )?;
        Ok(GnuCompatibilityProfile::gcc_4_2_1())
    }
}

fn validate_profile_set(
    manifest_path: &Path,
    field: &str,
    actual: &[String],
    expected: &[&str],
) -> Result<(), String> {
    let actual_set = actual.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if actual_set.len() != actual.len() {
        return Err(format!(
            "resource manifest {} contains duplicate hosted_header_profile.{field} entries",
            manifest_path.display()
        ));
    }
    let expected_set = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual_set != expected_set {
        let missing = expected_set
            .difference(&actual_set)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let unknown = actual_set
            .difference(&expected_set)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "resource manifest {} has an incompatible hosted_header_profile.{field}: missing [{missing}], unknown [{unknown}]",
            manifest_path.display()
        ));
    }
    Ok(())
}

fn checked_relative_path(value: &str, field: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "resource manifest {field} must be a normalized relative path, got {value:?}"
        ));
    }
    Ok(path.to_path_buf())
}

fn header_inventory(include: &Path) -> Result<BTreeSet<String>, String> {
    fn visit(root: &Path, directory: &Path, output: &mut BTreeSet<String>) -> Result<(), String> {
        let entries = fs::read_dir(directory).map_err(|error| {
            format!(
                "cannot inspect resource include directory {}: {error}",
                directory.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "cannot inspect an entry in resource include directory {}: {error}",
                    directory.display()
                )
            })?;
            let file_type = entry.file_type().map_err(|error| {
                format!(
                    "cannot inspect resource path {}: {error}",
                    entry.path().display()
                )
            })?;
            if file_type.is_dir() {
                visit(root, &entry.path(), output)?;
            } else if file_type.is_file() {
                let entry_path = entry.path();
                let relative = entry_path.strip_prefix(root).map_err(|error| {
                    format!(
                        "cannot normalize resource path {}: {error}",
                        entry_path.display()
                    )
                })?;
                output.insert(slash_path(relative));
            } else {
                return Err(format!(
                    "resource include inventory contains unsupported path {}",
                    entry.path().display()
                ));
            }
        }
        Ok(())
    }

    let mut output = BTreeSet::new();
    visit(include, include, &mut output)?;
    Ok(output)
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ManifestValue {
    Integer(u64),
    String(String),
    Strings(Vec<String>),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ManifestDocument {
    values: BTreeMap<String, ManifestValue>,
}

impl ManifestDocument {
    fn parse(source: &str) -> Result<Self, String> {
        let lines = source.lines().collect::<Vec<_>>();
        let mut values = BTreeMap::new();
        let mut section = String::new();
        let mut index = 0;

        while index < lines.len() {
            let line_number = index + 1;
            let line = strip_comment(lines[index]).trim();
            index += 1;
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                let Some(name) = line
                    .strip_prefix('[')
                    .and_then(|line| line.strip_suffix(']'))
                else {
                    return Err(format!("line {line_number}: malformed section header"));
                };
                if name.is_empty() || !name.chars().all(is_key_character) {
                    return Err(format!("line {line_number}: invalid section name {name:?}"));
                }
                section = name.to_owned();
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("line {line_number}: expected key = value"));
            };
            let key = key.trim();
            if key.is_empty() || !key.chars().all(is_key_character) {
                return Err(format!("line {line_number}: invalid key {key:?}"));
            }
            let mut value = value.trim().to_owned();
            if value.starts_with('[') && !contains_unquoted_array_end(&value) {
                while index < lines.len() {
                    let continuation = strip_comment(lines[index]).trim();
                    index += 1;
                    if !continuation.is_empty() {
                        value.push(' ');
                        value.push_str(continuation);
                    }
                    if contains_unquoted_array_end(&value) {
                        break;
                    }
                }
            }
            let parsed = parse_manifest_value(&value)
                .map_err(|error| format!("line {line_number}: {error}"))?;
            let full_key = if section.is_empty() {
                key.to_owned()
            } else {
                format!("{section}.{key}")
            };
            if values.insert(full_key.clone(), parsed).is_some() {
                return Err(format!("line {line_number}: duplicate key {full_key:?}"));
            }
        }

        Ok(Self { values })
    }

    fn integer(&mut self, key: &str) -> Result<u64, String> {
        match self.values.remove(key) {
            Some(ManifestValue::Integer(value)) => Ok(value),
            Some(_) => Err(format!("{key} must be an integer")),
            None => Err(format!("missing required field {key}")),
        }
    }

    fn string(&mut self, key: &str) -> Result<String, String> {
        match self.values.remove(key) {
            Some(ManifestValue::String(value)) => Ok(value),
            Some(_) => Err(format!("{key} must be a string")),
            None => Err(format!("missing required field {key}")),
        }
    }

    fn strings(&mut self, key: &str) -> Result<Vec<String>, String> {
        match self.values.remove(key) {
            Some(ManifestValue::Strings(value)) => Ok(value),
            Some(_) => Err(format!("{key} must be an array of strings")),
            None => Err(format!("missing required field {key}")),
        }
    }

    fn finish(self) -> Result<(), String> {
        if let Some(key) = self.values.keys().next() {
            Err(format!("unknown field {key}"))
        } else {
            Ok(())
        }
    }
}

fn is_key_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '-'
}

fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == '#' && !quoted {
            return &line[..index];
        }
    }
    line
}

fn contains_unquoted_array_end(value: &str) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == ']' && !quoted {
            return true;
        }
    }
    false
}

fn parse_manifest_value(value: &str) -> Result<ManifestValue, String> {
    let value = value.trim();
    if value.starts_with('"') {
        let (parsed, remainder) = parse_quoted(value)?;
        if !remainder.trim().is_empty() {
            return Err("unexpected text after string".to_owned());
        }
        Ok(ManifestValue::String(parsed))
    } else if value.starts_with('[') {
        parse_string_array(value).map(ManifestValue::Strings)
    } else {
        value
            .parse::<u64>()
            .map(ManifestValue::Integer)
            .map_err(|_| "expected an integer, string, or array of strings".to_owned())
    }
}

fn parse_string_array(value: &str) -> Result<Vec<String>, String> {
    let Some(mut remainder) = value.strip_prefix('[') else {
        return Err("expected an array".to_owned());
    };
    let mut output = Vec::new();
    loop {
        remainder = remainder.trim_start();
        if let Some(after) = remainder.strip_prefix(']') {
            if !after.trim().is_empty() {
                return Err("unexpected text after array".to_owned());
            }
            return Ok(output);
        }
        let (item, after) = parse_quoted(remainder)?;
        output.push(item);
        remainder = after.trim_start();
        if let Some(after_comma) = remainder.strip_prefix(',') {
            remainder = after_comma;
        } else if !remainder.starts_with(']') {
            return Err("expected ',' or ']' after array item".to_owned());
        }
    }
}

fn parse_quoted(value: &str) -> Result<(String, &str), String> {
    let Some(value) = value.strip_prefix('"') else {
        return Err("expected a quoted string".to_owned());
    };
    let mut output = String::new();
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            output.push(match character {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                _ => return Err(format!("unsupported escape sequence \\{character}")),
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            let remainder = &value[index + character.len_utf8()..];
            return Ok((output, remainder));
        } else {
            output.push(character);
        }
    }
    Err("unterminated quoted string".to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    const VALID_MANIFEST: &str = concat!(
        "format_version = 2\n",
        "compiler_version = \"",
        env!("CARGO_PKG_VERSION"),
        "\"\n",
        "\n",
        "[headers]\n",
        "directory = \"include\"\n",
        "compiler_owned = [\"stdbool.h\"]\n",
        "target_derived = [\"stddef.h\"]\n",
        "hosted_wrappers = [\"stdint.h\"]\n",
        "\n",
        "[hosted_header_profile]\n",
        "name = \"gcc-4.2.1\"\n",
        "version = \"4.2.1\"\n",
        "scope = \"code-generation\"\n",
        "selection_gate = \"__GNUC_PREREQ(4, 2)\"\n",
        "rationale = \"The conservative gate selects a tested preprocessing and declaration-parsing surface without implying newer GNU features.\"\n",
        "capabilities = [\n",
        "  \"computed-includes\",\n",
        "  \"function-like-macros\",\n",
        "  \"gcc-diagnostic-pragma\",\n",
        "  \"gcc-system-header-pragma\",\n",
        "  \"gnu-comma-elision\",\n",
        "  \"gnu-alternative-keywords\",\n",
        "  \"gnu-attribute-specifiers\",\n",
        "  \"gnu-declaration-asm-labels\",\n",
        "  \"gnu-extension-marker\",\n",
        "  \"gnu-named-variadic-macros\",\n",
        "  \"gnu-restrict-qualifiers\",\n",
        "  \"gnu-typeof\",\n",
        "  \"include-next\",\n",
        "  \"line-control\",\n",
        "  \"object-like-macros\",\n",
        "  \"pragma-operator\",\n",
        "  \"token-pasting\",\n",
        "  \"token-stringification\",\n",
        "  \"variadic-macros\",\n",
        "  \"warning-directive\",\n",
        "]\n",
        "declined_capabilities = [\"va-opt\"]\n",
    );

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("ccc-resource-test-{}-{id}", std::process::id()));
            fs::create_dir_all(path.join("include")).unwrap();
            fs::write(path.join("include/stdbool.h"), "#define bool _Bool\n").unwrap();
            fs::write(
                path.join("include/stddef.h"),
                "typedef __SIZE_TYPE__ size_t;\n",
            )
            .unwrap();
            fs::write(path.join("include/stdint.h"), "#include_next <stdint.h>\n").unwrap();
            Self(path)
        }

        fn write_manifest(&self, manifest: &str) {
            fs::write(self.0.join("manifest.toml"), manifest).unwrap();
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_the_development_resource_directory() {
        let resources = ResourceDirectory::discover(None).unwrap();
        assert!(resources.root().ends_with("resource-dir"));
        assert!(resources.include().join("stdbool.h").is_file());
        assert!(resources.include().join("stddef.h").is_file());
    }

    #[test]
    fn ships_the_target_derived_stddef_contract() {
        let resources = ResourceDirectory::discover(None).unwrap();
        let manifest_source = fs::read_to_string(resources.root().join("manifest.toml")).unwrap();
        let manifest = ResourceManifest::parse(&manifest_source).unwrap();
        assert_eq!(manifest.format_version, RESOURCE_FORMAT_VERSION);
        assert_eq!(manifest.headers.target_derived, vec!["stddef.h".to_owned()]);
        assert!(
            !manifest
                .headers
                .compiler_owned
                .iter()
                .any(|header| header == "stddef.h")
        );
        assert!(
            !manifest
                .headers
                .hosted_wrappers
                .iter()
                .any(|header| header == "stddef.h")
        );

        let stddef = fs::read_to_string(resources.include().join("stddef.h")).unwrap();
        for contract in [
            "typedef __SIZE_TYPE__ size_t;",
            "typedef __PTRDIFF_TYPE__ ptrdiff_t;",
            "max_align_t;",
            "#define offsetof(type, member) __builtin_offsetof(type, member)",
        ] {
            assert!(
                stddef.contains(contract),
                "stddef.h is missing contract {contract:?}"
            );
        }
    }

    #[test]
    fn ships_the_hosted_math_classification_wrapper() {
        let resources = ResourceDirectory::discover(None).unwrap();
        let manifest_source = fs::read_to_string(resources.root().join("manifest.toml")).unwrap();
        let manifest = ResourceManifest::parse(&manifest_source).unwrap();
        assert_eq!(manifest.headers.hosted_wrappers, vec!["math.h".to_owned()]);

        let math = fs::read_to_string(resources.include().join("math.h")).unwrap();
        for contract in [
            "#include_next <math.h>",
            "#define isfinite(value) __ccc_math_isfinite(value)",
            "#define isinf(value) __ccc_math_isinf(value)",
            "#define isnan(value) __ccc_math_isnan(value)",
        ] {
            assert!(
                math.contains(contract),
                "math.h is missing contract {contract:?}"
            );
        }
    }

    #[test]
    fn parses_and_validates_the_complete_manifest() {
        let directory = TestDirectory::new();
        directory.write_manifest(VALID_MANIFEST);

        let resources = ResourceDirectory::load(directory.0.clone()).unwrap();
        assert_eq!(resources.include(), directory.0.join("include"));
        assert_eq!(
            resources.hosted_header_profile(),
            &GnuCompatibilityProfile::gcc_4_2_1()
        );
    }

    #[test]
    fn rejects_malformed_and_incomplete_manifests() {
        let directory = TestDirectory::new();
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(error.contains("cannot read resource manifest"), "{error}");

        directory.write_manifest("format_version = nope\n");
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(error.contains("expected an integer"), "{error}");

        directory.write_manifest(
            &VALID_MANIFEST.replace("selection_gate = \"__GNUC_PREREQ(4, 2)\"\n", ""),
        );
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(error.contains("missing required field"), "{error}");
    }

    #[test]
    fn rejects_incompatible_format_and_compiler_versions() {
        let directory = TestDirectory::new();
        for incompatible in [1, 3] {
            directory.write_manifest(&VALID_MANIFEST.replacen(
                "format_version = 2",
                &format!("format_version = {incompatible}"),
                1,
            ));
            let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
            assert!(error.contains("expected 2"), "{error}");
        }

        directory.write_manifest(&VALID_MANIFEST.replacen(
            concat!("compiler_version = \"", env!("CARGO_PKG_VERSION"), "\""),
            "compiler_version = \"999.0.0\"",
            1,
        ));
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(
            error.contains(concat!("expected ", env!("CARGO_PKG_VERSION"))),
            "{error}"
        );
    }

    #[test]
    fn rejects_profile_drift() {
        let directory = TestDirectory::new();
        directory
            .write_manifest(&VALID_MANIFEST.replace("version = \"4.2.1\"", "version = \"9.0.0\""));

        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(error.contains("expected \"4.2.1\""), "{error}");
    }

    #[test]
    fn rejects_missing_and_unlisted_headers() {
        let directory = TestDirectory::new();
        directory.write_manifest(&VALID_MANIFEST.replace("stdbool.h", "missing.h"));
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(error.contains("lists missing header"), "{error}");

        directory.write_manifest(VALID_MANIFEST);
        fs::write(directory.0.join("include/unlisted.h"), "\n").unwrap();
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(error.contains("unlisted [unlisted.h]"), "{error}");
    }

    #[test]
    fn rejects_duplicate_or_overlapping_header_classes_and_unsafe_paths() {
        let directory = TestDirectory::new();
        directory.write_manifest(&VALID_MANIFEST.replace(
            "compiler_owned = [\"stdbool.h\"]",
            "compiler_owned = [\"stdbool.h\", \"stdbool.h\"]",
        ));
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(
            error.contains("more than once in headers.compiler_owned"),
            "{error}"
        );

        directory.write_manifest(&VALID_MANIFEST.replace(
            "target_derived = [\"stddef.h\"]",
            "target_derived = [\"stddef.h\", \"stdbool.h\"]",
        ));
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(
            error.contains("both headers.compiler_owned and headers.target_derived"),
            "{error}"
        );

        directory.write_manifest(
            &VALID_MANIFEST.replace("directory = \"include\"", "directory = \"../include\""),
        );
        let error = ResourceDirectory::load(directory.0.clone()).unwrap_err();
        assert!(error.contains("normalized relative path"), "{error}");
    }
}
