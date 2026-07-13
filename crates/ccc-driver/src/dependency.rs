use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MAKE_LINE_WIDTH: usize = 80;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DependencyRecord {
    pub path: PathBuf,
    pub is_system: bool,
}

impl DependencyRecord {
    pub(crate) fn new(path: impl Into<PathBuf>, is_system: bool) -> Self {
        Self {
            path: path.into(),
            is_system,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MakeTarget {
    /// Text supplied with `-MT`; Make syntax is preserved verbatim.
    Literal(String),
    /// Text supplied with `-MQ`; characters interpreted by Make are escaped.
    Quoted(String),
}

#[derive(Clone, Debug)]
pub(crate) struct DependencyRenderOptions<'a> {
    pub main_source: &'a Path,
    /// The object output used as the implicit target. When absent, the target
    /// is derived from the main source's basename.
    pub output_target: Option<&'a Path>,
    pub targets: &'a [MakeTarget],
    pub include_system_headers: bool,
    pub phony_targets: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RenderedDependencies {
    pub contents: String,
    /// The ordered, filtered prerequisites represented by `contents`.
    pub prerequisites: Vec<PathBuf>,
}

impl RenderedDependencies {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.contents.as_bytes()
    }
}

/// Render one deterministic Make rule and any requested header phony rules.
///
/// The main source is always the first prerequisite. Remaining records retain
/// first-seen order. System records are filtered before de-duplication so that
/// a later non-system occurrence of the same spelling is still represented.
pub(crate) fn render_dependencies(
    options: DependencyRenderOptions<'_>,
    dependencies: &[DependencyRecord],
) -> RenderedDependencies {
    let prerequisites = collect_prerequisites(
        options.main_source,
        dependencies,
        options.include_system_headers,
    );
    let targets = render_targets(&options);

    let mut contents = String::new();
    let mut column = 0;
    for target in targets {
        append_name(&mut contents, &mut column, &target, MAKE_LINE_WIDTH);
    }
    contents.push(':');
    column += 1;

    for prerequisite in &prerequisites {
        let quoted = make_quote(&path_text(prerequisite));
        append_name(&mut contents, &mut column, &quoted, MAKE_LINE_WIDTH);
    }
    contents.push('\n');

    if options.phony_targets {
        for prerequisite in prerequisites.iter().skip(1) {
            contents.push('\n');
            contents.push_str(&make_quote(&path_text(prerequisite)));
            contents.push_str(":\n");
        }
    }

    RenderedDependencies {
        contents,
        prerequisites,
    }
}

/// Choose the side-effect dependency file path used when `-MF` is absent.
///
/// An explicit object output retains its directory and has its final suffix
/// replaced with `.d`. Otherwise the source directory is discarded and the
/// source basename receives the `.d` suffix.
pub(crate) fn default_dependency_path(main_source: &Path, output_target: Option<&Path>) -> PathBuf {
    match output_target {
        Some(output) => replace_final_suffix(output, "d"),
        None => {
            let basename = main_source
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| main_source.to_path_buf());
            replace_final_suffix(&basename, "d")
        }
    }
}

fn collect_prerequisites(
    main_source: &Path,
    dependencies: &[DependencyRecord],
    include_system_headers: bool,
) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut prerequisites = Vec::with_capacity(dependencies.len() + 1);

    seen.insert(main_source.to_path_buf());
    prerequisites.push(main_source.to_path_buf());

    for dependency in dependencies {
        if dependency.is_system && !include_system_headers {
            continue;
        }
        if seen.insert(dependency.path.clone()) {
            prerequisites.push(dependency.path.clone());
        }
    }

    prerequisites
}

fn render_targets(options: &DependencyRenderOptions<'_>) -> Vec<String> {
    if !options.targets.is_empty() {
        return options
            .targets
            .iter()
            .map(|target| match target {
                MakeTarget::Literal(text) => text.clone(),
                MakeTarget::Quoted(text) => make_quote(text),
            })
            .collect();
    }

    let implicit = options
        .output_target
        .map_or_else(|| default_object_target(options.main_source), path_text);
    vec![make_quote(&implicit)]
}

fn default_object_target(source: &Path) -> String {
    let basename = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_text(source));
    replace_final_suffix_text(&basename, "o")
}

fn replace_final_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut result = path.to_path_buf();
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_text(path));
    result.set_file_name(replace_final_suffix_text(&filename, suffix));
    result
}

fn replace_final_suffix_text(name: &str, suffix: &str) -> String {
    match name.rfind('.') {
        Some(index) => format!("{}.{}", &name[..index], suffix),
        None => format!("{name}.{suffix}"),
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn append_name(output: &mut String, column: &mut usize, name: &str, line_width: usize) {
    if *column != 0 {
        if *column + name.len() > line_width {
            output.push_str(" \\\n");
            *column = 0;
        }
        output.push(' ');
        *column += 1;
    }
    output.push_str(name);
    *column += name.len();
}

/// Apply GCC-compatible Make quoting to a target or prerequisite spelling.
fn make_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len());
    let mut preceding_backslashes = 0;

    for character in text.chars() {
        match character {
            '\\' => {
                preceding_backslashes += 1;
            }
            '$' => {
                quoted.push('$');
                preceding_backslashes = 0;
            }
            ' ' | '\t' => {
                for _ in 0..preceding_backslashes {
                    quoted.push('\\');
                }
                quoted.push('\\');
                preceding_backslashes = 0;
            }
            '#' => {
                quoted.push('\\');
                preceding_backslashes = 0;
            }
            _ => {
                preceding_backslashes = 0;
            }
        }
        quoted.push(character);
    }

    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options<'a>(
        main_source: &'a Path,
        output_target: Option<&'a Path>,
        targets: &'a [MakeTarget],
    ) -> DependencyRenderOptions<'a> {
        DependencyRenderOptions {
            main_source,
            output_target,
            targets,
            include_system_headers: true,
            phony_targets: false,
        }
    }

    #[test]
    fn derives_and_quotes_the_implicit_target() {
        let rendered = render_dependencies(
            options(Path::new("source dir/main.c"), None, &[]),
            &[DependencyRecord::new("include/value.h", false)],
        );

        assert_eq!(
            rendered.contents,
            "main.o: source\\ dir/main.c include/value.h\n"
        );
        assert_eq!(
            rendered.prerequisites,
            [
                PathBuf::from("source dir/main.c"),
                PathBuf::from("include/value.h")
            ]
        );

        let rendered = render_dependencies(
            options(
                Path::new("main.c"),
                Some(Path::new("object dir/main$1.o")),
                &[],
            ),
            &[],
        );
        assert_eq!(rendered.contents, "object\\ dir/main$$1.o: main.c\n");
    }

    #[test]
    fn preserves_literal_targets_and_quotes_quoted_targets() {
        let targets = [
            MakeTarget::Literal("$(objects)one.o two.o".to_owned()),
            MakeTarget::Quoted("build/$three #.o".to_owned()),
        ];
        let rendered = render_dependencies(options(Path::new("main.c"), None, &targets), &[]);

        assert_eq!(
            rendered.contents,
            "$(objects)one.o two.o build/$$three\\ \\#.o: main.c\n"
        );
    }

    #[test]
    fn filters_before_deduplicating_and_retains_first_seen_order() {
        let mut render_options = options(Path::new("main.c"), None, &[]);
        render_options.include_system_headers = false;
        let rendered = render_dependencies(
            render_options,
            &[
                DependencyRecord::new("system.h", true),
                DependencyRecord::new("user.h", false),
                DependencyRecord::new("system.h", false),
                DependencyRecord::new("user.h", false),
                DependencyRecord::new("other.h", true),
                DependencyRecord::new("main.c", false),
            ],
        );

        assert_eq!(rendered.contents, "main.o: main.c user.h system.h\n");
        assert_eq!(
            rendered.prerequisites,
            [
                PathBuf::from("main.c"),
                PathBuf::from("user.h"),
                PathBuf::from("system.h")
            ]
        );
    }

    #[test]
    fn phony_rules_cover_each_retained_header_once() {
        let mut render_options = options(Path::new("main.c"), None, &[]);
        render_options.include_system_headers = false;
        render_options.phony_targets = true;
        let rendered = render_dependencies(
            render_options,
            &[
                DependencyRecord::new("include/one header.h", false),
                DependencyRecord::new("include/one header.h", false),
                DependencyRecord::new("sdk/system.h", true),
                DependencyRecord::new("include/two#header.h", false),
            ],
        );

        assert_eq!(
            rendered.contents,
            concat!(
                "main.o: main.c include/one\\ header.h include/two\\#header.h\n",
                "\ninclude/one\\ header.h:\n",
                "\ninclude/two\\#header.h:\n",
            )
        );
    }

    #[test]
    fn make_quoting_handles_dollars_hashes_and_backslash_whitespace() {
        assert_eq!(make_quote("$(obj)/a b#c"), "$$(obj)/a\\ b\\#c");
        assert_eq!(make_quote(r"one\ two"), r"one\\\ two");
        assert_eq!(make_quote("one\ttwo"), "one\\\ttwo");
    }

    #[test]
    fn derives_default_dependency_paths() {
        assert_eq!(
            default_dependency_path(Path::new("source/hello.c"), None),
            PathBuf::from("hello.d")
        );
        assert_eq!(
            default_dependency_path(
                Path::new("source/hello.c"),
                Some(Path::new("build/hello.object"))
            ),
            PathBuf::from("build/hello.d")
        );
        assert_eq!(
            default_dependency_path(Path::new("source/README"), None),
            PathBuf::from("README.d")
        );
    }

    #[test]
    fn wraps_long_rules_with_a_stable_make_continuation() {
        let rendered = render_dependencies(
            options(Path::new("main.c"), None, &[]),
            &[DependencyRecord::new(
                "include/a-very-long-directory-name/and-an-equally-long-header-name.h",
                false,
            )],
        );

        assert_eq!(
            rendered.contents,
            concat!(
                "main.o: main.c \\\n",
                " include/a-very-long-directory-name/and-an-equally-long-header-name.h\n",
            )
        );
    }

    #[test]
    fn exposes_contents_for_atomic_writes() {
        let rendered = render_dependencies(options(Path::new("main.c"), None, &[]), &[]);
        assert_eq!(rendered.as_bytes(), b"main.o: main.c\n");
    }
}
