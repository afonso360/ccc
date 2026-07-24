use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SCHEMA_VERSION: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    Preprocessing,
    Parsing,
    SemanticAnalysis,
    CccIrLowering,
    CccIrOptimization,
    CodegenTotal,
    ObjectPackaging,
    Pipeline,
}

impl Phase {
    const ORDERED: [Self; 8] = [
        Self::Preprocessing,
        Self::Parsing,
        Self::SemanticAnalysis,
        Self::CccIrLowering,
        Self::CccIrOptimization,
        Self::CodegenTotal,
        Self::ObjectPackaging,
        Self::Pipeline,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Preprocessing => 0,
            Self::Parsing => 1,
            Self::SemanticAnalysis => 2,
            Self::CccIrLowering => 3,
            Self::CccIrOptimization => 4,
            Self::CodegenTotal => 5,
            Self::ObjectPackaging => 6,
            Self::Pipeline => 7,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Preprocessing => "preprocessing",
            Self::Parsing => "parsing",
            Self::SemanticAnalysis => "semantic_analysis",
            Self::CccIrLowering => "ccc_ir_lowering",
            Self::CccIrOptimization => "ccc_ir_optimization",
            Self::CodegenTotal => "codegen.total",
            Self::ObjectPackaging => "object_packaging",
            Self::Pipeline => "pipeline",
        }
    }
}

/// Wall-clock phase measurements for one driver translation.
///
/// The driver creates this value only when the timing option is present.
/// Untimed invocations therefore perform no clock reads and allocate no
/// recorder.
pub(crate) struct PhaseTimings {
    pipeline_started: Instant,
    elapsed: [Option<Duration>; Phase::ORDERED.len()],
    protected_inputs: Vec<PathBuf>,
}

impl PhaseTimings {
    pub(crate) fn start_pipeline() -> Self {
        Self {
            pipeline_started: Instant::now(),
            elapsed: [None; Phase::ORDERED.len()],
            protected_inputs: Vec::new(),
        }
    }

    pub(crate) fn begin(timings: Option<&Self>) -> Option<Instant> {
        timings.map(|_| Instant::now())
    }

    pub(crate) fn finish(timings: Option<&mut Self>, phase: Phase, started: Option<Instant>) {
        if let (Some(timings), Some(started)) = (timings, started) {
            timings.record(phase, started.elapsed());
        }
    }

    pub(crate) fn finish_pipeline(&mut self) {
        self.record(Phase::Pipeline, self.pipeline_started.elapsed());
    }

    pub(crate) fn protect_inputs<'a>(&mut self, paths: impl IntoIterator<Item = &'a Path>) {
        self.protected_inputs
            .extend(paths.into_iter().map(Path::to_path_buf));
    }

    pub(crate) fn protected_inputs(&self) -> impl Iterator<Item = &Path> {
        self.protected_inputs.iter().map(PathBuf::as_path)
    }

    fn record(&mut self, phase: Phase, elapsed: Duration) {
        let slot = &mut self.elapsed[phase.index()];
        debug_assert!(slot.is_none(), "phase {} was recorded twice", phase.name());
        *slot = Some(elapsed);
    }

    fn render(&self) -> String {
        let mut output = String::new();
        writeln!(output, "schema_version\t{SCHEMA_VERSION}")
            .expect("writing phase timings to a String cannot fail");
        for phase in Phase::ORDERED {
            if let Some(elapsed) = self.elapsed[phase.index()] {
                writeln!(output, "{}\t{}", phase.name(), elapsed.as_nanos())
                    .expect("writing phase timings to a String cannot fail");
            }
        }
        output
    }

    pub(crate) fn publish(&self, destination: &Path) -> io::Result<()> {
        let rendered = self.render();
        crate::atomic_output::write_atomic(destination, rendered.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::Ordering;

    use super::*;

    fn test_timings() -> PhaseTimings {
        PhaseTimings {
            pipeline_started: Instant::now(),
            elapsed: [None; Phase::ORDERED.len()],
            protected_inputs: Vec::new(),
        }
    }

    fn test_directory(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ccc-phase-timing-{}-{}-{name}",
            std::process::id(),
            crate::TEMPORARY_ID.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn renderer_is_versioned_ordered_numeric_and_omits_unrecorded_phases() {
        let mut timings = test_timings();
        timings.record(Phase::Preprocessing, Duration::from_nanos(11));
        timings.record(Phase::SemanticAnalysis, Duration::from_nanos(13));
        timings.record(Phase::CodegenTotal, Duration::from_nanos(17));
        timings.record(Phase::Pipeline, Duration::from_nanos(19));

        assert_eq!(
            timings.render(),
            "schema_version\t1\n\
             preprocessing\t11\n\
             semantic_analysis\t13\n\
             codegen.total\t17\n\
             pipeline\t19\n"
        );
    }

    #[test]
    fn pipeline_measurement_is_frozen_before_report_serialization() {
        let mut timings = PhaseTimings::start_pipeline();
        timings.finish_pipeline();
        let measured = timings.elapsed[Phase::Pipeline.index()].unwrap();

        std::thread::sleep(Duration::from_millis(2));
        let rendered = timings.render();
        let reported = rendered
            .lines()
            .find_map(|row| row.strip_prefix("pipeline\t"))
            .unwrap()
            .parse::<u128>()
            .unwrap();

        assert_eq!(reported, measured.as_nanos());
    }

    #[test]
    fn failed_atomic_publication_leaves_the_destination_untouched() {
        let directory = test_directory("failure");
        let destination = directory.join("timings.tsv");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("marker"), "old").unwrap();
        let mut timings = test_timings();
        timings.record(Phase::Pipeline, Duration::from_nanos(1));

        assert!(timings.publish(&destination).is_err());
        assert_eq!(
            fs::read_to_string(destination.join("marker")).unwrap(),
            "old"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn successful_atomic_publication_replaces_an_old_sidecar() {
        let directory = test_directory("success");
        let destination = directory.join("timings.tsv");
        fs::write(&destination, "old").unwrap();
        let mut timings = test_timings();
        timings.record(Phase::Pipeline, Duration::from_nanos(23));

        timings.publish(&destination).unwrap();
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "schema_version\t1\npipeline\t23\n"
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        fs::remove_dir_all(directory).unwrap();
    }
}
