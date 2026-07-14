use std::collections::{BTreeMap, BTreeSet};

use ccc_target::PackingPolicy;
use ccc_types::{ArrayLength, ArrayType, Field, RecordKind, TypeId, TypeStore};
use sha2::{Digest as _, Sha256};

use crate::{AbiClass, PassingMode, hex};

pub const CLASSIFIER_CORPUS_SEED: u64 = 0x4343_435f_4142_4931;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CorpusReturnMode {
    Direct,
    Indirect,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CorpusSizeBucket {
    OneEightbyte,
    TwoEightbytes,
    Memory,
}

/// A canonical type recipe shared by the in-process classifier gate and the
/// native cross-link fixture generator.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CorpusFixture {
    IntegerBytes(u8),
    FloatRecord,
    DoubleArray(u8),
    IntegerPair,
    MixedSseInteger,
    MixedIntegerSse,
    UnionMerge,
    PackedAlignedNine,
    PackedUnalignedInteger,
    CrossingBitfield,
    NestedUnionAndDouble,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CorpusLeaf {
    pub offset: u8,
    pub size: u8,
    pub class: AbiClass,
}

/// Scalar arguments surrounding the aggregate under test. These counts are
/// part of the real function signature so every corpus entry exercises a
/// distinct classifier/allocator input, including register exhaustion and
/// trailing stack allocation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CorpusAllocationPattern {
    pub leading_integer: u8,
    pub leading_sse: u8,
    pub trailing_integer: u8,
    pub trailing_sse: u8,
}

impl CorpusAllocationPattern {
    const EMPTY: Self = Self {
        leading_integer: 0,
        leading_sse: 0,
        trailing_integer: 0,
        trailing_sse: 0,
    };

    fn from_index(mut index: u64) -> Self {
        let leading_integer = (index % 7) as u8;
        index /= 7;
        let leading_sse = (index % 9) as u8;
        index /= 9;
        let trailing_integer = (index % 7) as u8;
        index /= 7;
        let trailing_sse = (index % 9) as u8;
        Self {
            leading_integer,
            leading_sse,
            trailing_integer,
            trailing_sse,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CorpusCase {
    pub id: String,
    pub name: Option<&'static str>,
    pub fixture: CorpusFixture,
    pub size: u8,
    pub align: u8,
    pub leaves: Vec<CorpusLeaf>,
    pub passing: PassingMode,
    pub return_mode: CorpusReturnMode,
    pub packed: bool,
    pub mixed_class: bool,
    pub allocation: CorpusAllocationPattern,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CorpusBucket {
    passing: u8,
    return_mode: CorpusReturnMode,
    size: CorpusSizeBucket,
    packed: bool,
    mixed_class: bool,
}

impl CorpusCase {
    pub fn bucket(&self) -> CorpusBucket {
        CorpusBucket {
            passing: match self.passing {
                PassingMode::Registers | PassingMode::Scalar => 0,
                PassingMode::Memory => 1,
                PassingMode::Void => 2,
            },
            return_mode: self.return_mode,
            size: match self.size {
                0..=8 => CorpusSizeBucket::OneEightbyte,
                9..=16 => CorpusSizeBucket::TwoEightbytes,
                _ => CorpusSizeBucket::Memory,
            },
            packed: self.packed,
            mixed_class: self.mixed_class,
        }
    }

    pub fn canonical_sha256(&self) -> [u8; 32] {
        let mut encoder = Vec::new();
        encoder.extend_from_slice(b"ccc-classifier-corpus-case-v3\0");
        encoder.extend_from_slice(&CLASSIFIER_CORPUS_SEED.to_le_bytes());
        encode_fixture(self.fixture, &mut encoder);
        encoder.extend_from_slice(&[
            self.size,
            self.align,
            self.passing as u8,
            self.return_mode as u8,
            u8::from(self.packed),
            u8::from(self.mixed_class),
            self.allocation.leading_integer,
            self.allocation.leading_sse,
            self.allocation.trailing_integer,
            self.allocation.trailing_sse,
        ]);
        encoder.extend_from_slice(&(self.leaves.len() as u32).to_le_bytes());
        for leaf in &self.leaves {
            encoder.extend_from_slice(&[leaf.offset, leaf.size, leaf.class as u8]);
        }
        Sha256::digest(encoder).into()
    }

    /// Materializes the exact canonical type consumed by the real classifier.
    pub fn materialize(&self, types: &mut TypeStore) -> TypeId {
        materialize_fixture(types, self.fixture)
    }

    /// Returns a standalone C declaration for native oracle generation.
    pub fn c_declaration(&self, tag: &str) -> String {
        match self.fixture {
            CorpusFixture::IntegerBytes(length) => {
                format!("struct {tag} {{ char bytes[{length}]; }};")
            }
            CorpusFixture::FloatRecord => format!("struct {tag} {{ float value; }};"),
            CorpusFixture::DoubleArray(length) => {
                format!("struct {tag} {{ double values[{length}]; }};")
            }
            CorpusFixture::IntegerPair => {
                format!("struct {tag} {{ long first; long second; }};")
            }
            CorpusFixture::MixedSseInteger => {
                format!("struct {tag} {{ double floating; long integer; }};")
            }
            CorpusFixture::MixedIntegerSse => {
                format!("struct {tag} {{ long integer; double floating; }};")
            }
            CorpusFixture::UnionMerge => {
                format!("union {tag} {{ long integer; double floating; }};")
            }
            CorpusFixture::PackedAlignedNine => format!(
                "#pragma pack(push, 1)\nstruct {tag} {{ long integer; char tail; }};\n#pragma pack(pop)"
            ),
            CorpusFixture::PackedUnalignedInteger => format!(
                "#pragma pack(push, 1)\nstruct {tag} {{ char prefix; int integer; }};\n#pragma pack(pop)"
            ),
            CorpusFixture::CrossingBitfield => format!(
                "#pragma pack(push, 1)\nstruct {tag} {{ char prefix[7]; unsigned long bits : 16; }};\n#pragma pack(pop)"
            ),
            CorpusFixture::NestedUnionAndDouble => format!(
                "struct {tag} {{ union {{ long integer; double floating; }} nested; double tail; }};"
            ),
        }
    }

    pub fn c_type_name(&self, tag: &str) -> String {
        if self.fixture == CorpusFixture::UnionMerge {
            format!("union {tag}")
        } else {
            format!("struct {tag}")
        }
    }
}

/// Returns 4,096 deterministic canonical plan inputs. The gate crosses 45
/// aggregate type recipes from 11 structural families with leading/trailing
/// GP and SSE allocator-pressure patterns; it does not claim 4,096 unrelated
/// aggregate layouts. Every recipe is materialized into `TypeStore` and every
/// complete signature is classified by the unit gate below.
pub fn classifier_corpus() -> Vec<CorpusCase> {
    let mut cases = named_cases();
    let mut canonical_hashes = cases
        .iter()
        .map(CorpusCase::canonical_sha256)
        .collect::<BTreeSet<_>>();
    assert_eq!(canonical_hashes.len(), cases.len());
    let mut state = CLASSIFIER_CORPUS_SEED;
    while cases.len() < 4096 {
        let fixture = fixture_from_selector(next_random(&mut state));
        let allocation = CorpusAllocationPattern::from_index(next_random(&mut state));
        let mut case = case_for_fixture(fixture, None, allocation);
        let canonical_hash = case.canonical_sha256();
        if !canonical_hashes.insert(canonical_hash) {
            continue;
        }
        case.id = format!("g{}", hex(&canonical_hash));
        cases.push(case);
    }
    assert_eq!(canonical_hashes.len(), 4096);
    cases
}

/// Selects 256 stable native-oracle fixtures by canonical hash within buckets.
pub fn selected_cross_link_cases() -> Vec<CorpusCase> {
    let corpus = classifier_corpus();
    let mut selected = corpus
        .iter()
        .filter(|case| case.name.is_some())
        .cloned()
        .collect::<Vec<_>>();
    let mut selected_hashes = selected
        .iter()
        .map(CorpusCase::canonical_sha256)
        .collect::<BTreeSet<_>>();
    let mut buckets = BTreeMap::<CorpusBucket, Vec<([u8; 32], CorpusCase)>>::new();
    for case in corpus.into_iter().filter(|case| case.name.is_none()) {
        buckets
            .entry(case.bucket())
            .or_default()
            .push((case.canonical_sha256(), case));
    }
    for cases in buckets.values_mut() {
        cases.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
    }
    let keys = buckets.keys().copied().collect::<Vec<_>>();
    let mut positions = BTreeMap::<CorpusBucket, usize>::new();
    while selected.len() < 256 {
        let before = selected.len();
        for key in &keys {
            let position = positions.entry(*key).or_default();
            let cases = &buckets[key];
            while let Some((canonical_hash, candidate)) = cases.get(*position) {
                *position += 1;
                if selected_hashes.insert(*canonical_hash) {
                    selected.push(candidate.clone());
                    break;
                }
            }
            if selected.len() == 256 {
                break;
            }
        }
        assert_ne!(
            before,
            selected.len(),
            "classifier corpus has fewer than 256 unique cases"
        );
    }
    selected
}

fn named_cases() -> Vec<CorpusCase> {
    [
        (
            "integer-one",
            CorpusFixture::IntegerBytes(8),
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "sse-one",
            CorpusFixture::DoubleArray(1),
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "integer-pair",
            CorpusFixture::IntegerPair,
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "sse-pair",
            CorpusFixture::DoubleArray(2),
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "mixed-sse-integer",
            CorpusFixture::MixedSseInteger,
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "mixed-integer-sse",
            CorpusFixture::MixedIntegerSse,
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "packed-unaligned",
            CorpusFixture::PackedUnalignedInteger,
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "large-memory",
            CorpusFixture::IntegerBytes(24),
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "gp-rollback",
            CorpusFixture::IntegerPair,
            CorpusAllocationPattern {
                leading_integer: 5,
                ..CorpusAllocationPattern::EMPTY
            },
        ),
        (
            "sse-rollback",
            CorpusFixture::DoubleArray(2),
            CorpusAllocationPattern {
                leading_sse: 7,
                ..CorpusAllocationPattern::EMPTY
            },
        ),
        (
            "mixed-rollback",
            CorpusFixture::MixedIntegerSse,
            CorpusAllocationPattern {
                leading_integer: 6,
                ..CorpusAllocationPattern::EMPTY
            },
        ),
        (
            "hidden-return",
            CorpusFixture::IntegerBytes(24),
            CorpusAllocationPattern {
                trailing_integer: 1,
                ..CorpusAllocationPattern::EMPTY
            },
        ),
        (
            "partial-integer",
            CorpusFixture::IntegerBytes(5),
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "partial-sse",
            CorpusFixture::FloatRecord,
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "union-merge",
            CorpusFixture::UnionMerge,
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "nested-two-eightbytes",
            CorpusFixture::NestedUnionAndDouble,
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "packed-aligned-nine",
            CorpusFixture::PackedAlignedNine,
            CorpusAllocationPattern::EMPTY,
        ),
        (
            "crossing-bitfield",
            CorpusFixture::CrossingBitfield,
            CorpusAllocationPattern::EMPTY,
        ),
    ]
    .into_iter()
    .map(|(name, fixture, allocation)| case_for_fixture(fixture, Some(name), allocation))
    .collect()
}

fn case_for_fixture(
    fixture: CorpusFixture,
    name: Option<&'static str>,
    allocation: CorpusAllocationPattern,
) -> CorpusCase {
    let (size, align, classes, packed) = expected_fixture(fixture);
    let passing = if size > 16 || classes == [AbiClass::Memory] {
        PassingMode::Memory
    } else {
        PassingMode::Registers
    };
    let leaves = if passing == PassingMode::Memory {
        memory_leaves(size)
    } else {
        classes
            .iter()
            .copied()
            .enumerate()
            .map(|(index, class)| CorpusLeaf {
                offset: (index * 8) as u8,
                size: size.saturating_sub((index * 8) as u8).min(8),
                class,
            })
            .collect()
    };
    let mixed_class = classes.contains(&AbiClass::Integer) && classes.contains(&AbiClass::Sse);
    CorpusCase {
        id: name.map_or_else(String::new, |name| format!("n{name}")),
        name,
        fixture,
        size,
        align,
        leaves,
        passing,
        return_mode: if passing == PassingMode::Memory {
            CorpusReturnMode::Indirect
        } else {
            CorpusReturnMode::Direct
        },
        packed,
        mixed_class,
        allocation,
    }
}

fn fixture_from_selector(selector: u64) -> CorpusFixture {
    match selector % 45 {
        value @ 0..=31 => CorpusFixture::IntegerBytes(value as u8 + 1),
        32 => CorpusFixture::FloatRecord,
        value @ 33..=36 => CorpusFixture::DoubleArray(value as u8 - 32),
        37 => CorpusFixture::IntegerPair,
        38 => CorpusFixture::MixedSseInteger,
        39 => CorpusFixture::MixedIntegerSse,
        40 => CorpusFixture::UnionMerge,
        41 => CorpusFixture::PackedAlignedNine,
        42 => CorpusFixture::PackedUnalignedInteger,
        43 => CorpusFixture::CrossingBitfield,
        44 => CorpusFixture::NestedUnionAndDouble,
        _ => unreachable!(),
    }
}

fn expected_fixture(fixture: CorpusFixture) -> (u8, u8, Vec<AbiClass>, bool) {
    match fixture {
        CorpusFixture::IntegerBytes(size) if size > 16 => (size, 1, vec![AbiClass::Memory], false),
        CorpusFixture::IntegerBytes(size) => (
            size,
            1,
            vec![AbiClass::Integer; usize::from(size.div_ceil(8))],
            false,
        ),
        CorpusFixture::FloatRecord => (4, 4, vec![AbiClass::Sse], false),
        CorpusFixture::DoubleArray(length) if length > 2 => {
            (length * 8, 8, vec![AbiClass::Memory], false)
        }
        CorpusFixture::DoubleArray(length) => (
            length * 8,
            8,
            vec![AbiClass::Sse; usize::from(length)],
            false,
        ),
        CorpusFixture::IntegerPair => (16, 8, vec![AbiClass::Integer, AbiClass::Integer], false),
        CorpusFixture::MixedSseInteger => (16, 8, vec![AbiClass::Sse, AbiClass::Integer], false),
        CorpusFixture::MixedIntegerSse => (16, 8, vec![AbiClass::Integer, AbiClass::Sse], false),
        CorpusFixture::UnionMerge => (8, 8, vec![AbiClass::Integer], false),
        CorpusFixture::PackedAlignedNine => {
            (9, 1, vec![AbiClass::Integer, AbiClass::Integer], true)
        }
        CorpusFixture::PackedUnalignedInteger => (5, 1, vec![AbiClass::Memory], true),
        CorpusFixture::CrossingBitfield => (9, 1, vec![AbiClass::Integer, AbiClass::Integer], true),
        CorpusFixture::NestedUnionAndDouble => {
            (16, 8, vec![AbiClass::Integer, AbiClass::Sse], false)
        }
    }
}

fn materialize_fixture(types: &mut TypeStore, fixture: CorpusFixture) -> TypeId {
    match fixture {
        CorpusFixture::IntegerBytes(length) => {
            let bytes = types.array(ArrayType {
                element: TypeId::CHAR.into(),
                length: ArrayLength::Constant(u64::from(length)),
            });
            record(
                types,
                RecordKind::Struct,
                vec![Field::named("bytes", bytes)],
                false,
            )
        }
        CorpusFixture::FloatRecord => record(
            types,
            RecordKind::Struct,
            vec![Field::named("value", TypeId::FLOAT)],
            false,
        ),
        CorpusFixture::DoubleArray(length) => {
            let values = types.array(ArrayType {
                element: TypeId::DOUBLE.into(),
                length: ArrayLength::Constant(u64::from(length)),
            });
            record(
                types,
                RecordKind::Struct,
                vec![Field::named("values", values)],
                false,
            )
        }
        CorpusFixture::IntegerPair => record(
            types,
            RecordKind::Struct,
            vec![
                Field::named("first", TypeId::LONG),
                Field::named("second", TypeId::LONG),
            ],
            false,
        ),
        CorpusFixture::MixedSseInteger => record(
            types,
            RecordKind::Struct,
            vec![
                Field::named("floating", TypeId::DOUBLE),
                Field::named("integer", TypeId::LONG),
            ],
            false,
        ),
        CorpusFixture::MixedIntegerSse => record(
            types,
            RecordKind::Struct,
            vec![
                Field::named("integer", TypeId::LONG),
                Field::named("floating", TypeId::DOUBLE),
            ],
            false,
        ),
        CorpusFixture::UnionMerge => record(
            types,
            RecordKind::Union,
            vec![
                Field::named("integer", TypeId::LONG),
                Field::named("floating", TypeId::DOUBLE),
            ],
            false,
        ),
        CorpusFixture::PackedAlignedNine => record(
            types,
            RecordKind::Struct,
            vec![
                Field::named("integer", TypeId::LONG),
                Field::named("tail", TypeId::CHAR),
            ],
            true,
        ),
        CorpusFixture::PackedUnalignedInteger => record(
            types,
            RecordKind::Struct,
            vec![
                Field::named("prefix", TypeId::CHAR),
                Field::named("integer", TypeId::INT),
            ],
            true,
        ),
        CorpusFixture::CrossingBitfield => {
            let prefix = types.array(ArrayType {
                element: TypeId::CHAR.into(),
                length: ArrayLength::Constant(7),
            });
            record(
                types,
                RecordKind::Struct,
                vec![
                    Field::named("prefix", prefix),
                    Field::bitfield(Some("bits".to_owned()), TypeId::UNSIGNED_LONG, 16),
                ],
                true,
            )
        }
        CorpusFixture::NestedUnionAndDouble => {
            let nested = record(
                types,
                RecordKind::Union,
                vec![
                    Field::named("integer", TypeId::LONG),
                    Field::named("floating", TypeId::DOUBLE),
                ],
                false,
            );
            record(
                types,
                RecordKind::Struct,
                vec![
                    Field::named("nested", nested),
                    Field::named("tail", TypeId::DOUBLE),
                ],
                false,
            )
        }
    }
}

fn record(types: &mut TypeStore, kind: RecordKind, fields: Vec<Field>, packed: bool) -> TypeId {
    let (id, ty) = types.declare_record(kind, None);
    types
        .complete_record_with_packing(
            id,
            fields,
            if packed {
                PackingPolicy::PACKED
            } else {
                PackingPolicy::NATIVE
            },
        )
        .unwrap();
    ty
}

fn memory_leaves(size: u8) -> Vec<CorpusLeaf> {
    (0..size.div_ceil(8))
        .map(|index| CorpusLeaf {
            offset: index * 8,
            size: size.saturating_sub(index * 8).min(8),
            class: AbiClass::Memory,
        })
        .collect()
}

fn encode_fixture(fixture: CorpusFixture, output: &mut Vec<u8>) {
    match fixture {
        CorpusFixture::IntegerBytes(value) => output.extend_from_slice(&[0, value]),
        CorpusFixture::FloatRecord => output.push(1),
        CorpusFixture::DoubleArray(value) => output.extend_from_slice(&[2, value]),
        CorpusFixture::IntegerPair => output.push(3),
        CorpusFixture::MixedSseInteger => output.push(4),
        CorpusFixture::MixedIntegerSse => output.push(5),
        CorpusFixture::UnionMerge => output.push(6),
        CorpusFixture::PackedAlignedNine => output.push(7),
        CorpusFixture::PackedUnalignedInteger => output.push(8),
        CorpusFixture::CrossingBitfield => output.push(9),
        CorpusFixture::NestedUnionAndDouble => output.push(10),
    }
}

fn next_random(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}

#[cfg(test)]
mod tests {
    use ccc_target::EffectiveCompilationConfig;
    use ccc_types::FunctionType;

    use super::*;
    use crate::{NativeResultPlan, classify_type, plan_function_type};

    #[test]
    fn corpus_executes_the_real_classifier_and_allocator() {
        let config = EffectiveCompilationConfig::default();
        let corpus = classifier_corpus();
        assert_eq!(corpus.len(), 4096);
        for case in corpus {
            let mut types = TypeStore::default();
            let ty = case.materialize(&mut types);
            let classified = classify_type(&types, ty, &config).unwrap();
            assert_eq!(classified.size, u64::from(case.size), "{}", case.id);
            assert_eq!(classified.align, u64::from(case.align), "{}", case.id);
            assert_eq!(classified.passing, case.passing, "{}", case.id);
            let expected_classes = if case.passing == PassingMode::Memory {
                vec![AbiClass::Memory]
            } else {
                case.leaves
                    .iter()
                    .map(|leaf| leaf.class)
                    .collect::<Vec<_>>()
            };
            assert_eq!(classified.classes, expected_classes, "{}", case.id);
            assert_eq!(
                classified
                    .pieces
                    .iter()
                    .map(|piece| (piece.offset as u8, piece.valid_bytes, piece.class))
                    .collect::<Vec<_>>(),
                case.leaves
                    .iter()
                    .map(|leaf| (leaf.offset, leaf.size, leaf.class))
                    .collect::<Vec<_>>(),
                "{}",
                case.id
            );

            let leading_gp = usize::from(case.allocation.leading_integer);
            let leading_sse = usize::from(case.allocation.leading_sse);
            let trailing_gp = usize::from(case.allocation.trailing_integer);
            let trailing_sse = usize::from(case.allocation.trailing_sse);
            let mut parameters = vec![TypeId::LONG.into(); leading_gp];
            parameters.extend(vec![
                ccc_types::QualifiedType::from(TypeId::DOUBLE);
                leading_sse
            ]);
            let aggregate_index = parameters.len();
            parameters.push(ty.into());
            parameters.extend(vec![
                ccc_types::QualifiedType::from(TypeId::LONG);
                trailing_gp
            ]);
            parameters.extend(vec![
                ccc_types::QualifiedType::from(TypeId::DOUBLE);
                trailing_sse
            ]);
            let signature = types.function_type(FunctionType::prototype(TypeId::VOID, parameters));
            let plan = plan_function_type(&types, signature, &config).unwrap();
            assert_eq!(
                plan.parameters.len(),
                leading_gp + leading_sse + 1 + trailing_gp + trailing_sse,
                "{}",
                case.id
            );
            let needed_gp = case
                .leaves
                .iter()
                .filter(|leaf| leaf.class == AbiClass::Integer)
                .count();
            let needed_sse = case
                .leaves
                .iter()
                .filter(|leaf| leaf.class == AbiClass::Sse)
                .count();
            let fits = case.passing != PassingMode::Memory
                && leading_gp + needed_gp <= 6
                && leading_sse + needed_sse <= 8;
            assert_eq!(
                plan.parameters[aggregate_index].classified.passing,
                if fits {
                    PassingMode::Registers
                } else {
                    PassingMode::Memory
                },
                "{}",
                case.id
            );

            let result_signature = types.function_type(FunctionType::prototype(ty, Vec::new()));
            let result = plan_function_type(&types, result_signature, &config).unwrap();
            assert_eq!(
                matches!(result.result, NativeResultPlan::Indirect { .. }),
                case.return_mode == CorpusReturnMode::Indirect,
                "{}",
                case.id
            );
        }
    }

    #[test]
    fn subset_is_exact_deterministic_and_has_native_declarations() {
        assert_eq!(classifier_corpus(), classifier_corpus());
        let corpus = classifier_corpus();
        assert_eq!(
            corpus
                .iter()
                .map(CorpusCase::canonical_sha256)
                .collect::<BTreeSet<_>>()
                .len(),
            4096
        );
        assert!(
            corpus
                .iter()
                .filter(|case| case.name.is_none())
                .all(|case| {
                    case.id == format!("g{}", hex(&case.canonical_sha256()))
                        && case.allocation.leading_integer <= 6
                        && case.allocation.leading_sse <= 8
                        && case.allocation.trailing_integer <= 6
                        && case.allocation.trailing_sse <= 8
                })
        );
        let selected = selected_cross_link_cases();
        assert_eq!(selected.len(), 256);
        assert_eq!(selected, selected_cross_link_cases());
        assert_eq!(
            selected
                .iter()
                .map(CorpusCase::canonical_sha256)
                .collect::<BTreeSet<_>>()
                .len(),
            256
        );
        assert!(
            selected
                .iter()
                .all(|case| case.c_declaration("Oracle").contains("Oracle"))
        );
        let covered = selected
            .iter()
            .map(CorpusCase::bucket)
            .collect::<BTreeSet<_>>();
        let declared = classifier_corpus()
            .iter()
            .map(CorpusCase::bucket)
            .collect::<BTreeSet<_>>();
        assert_eq!(covered, declared);
    }

    #[test]
    fn corpus_crosses_type_recipes_with_allocator_pressure() {
        let corpus = classifier_corpus();
        let type_recipes = corpus
            .iter()
            .map(|case| case.fixture)
            .collect::<BTreeSet<_>>();
        assert_eq!(type_recipes.len(), 45);

        let structural_families = type_recipes
            .iter()
            .map(|fixture| match fixture {
                CorpusFixture::IntegerBytes(_) => 0,
                CorpusFixture::FloatRecord => 1,
                CorpusFixture::DoubleArray(_) => 2,
                CorpusFixture::IntegerPair => 3,
                CorpusFixture::MixedSseInteger => 4,
                CorpusFixture::MixedIntegerSse => 5,
                CorpusFixture::UnionMerge => 6,
                CorpusFixture::PackedAlignedNine => 7,
                CorpusFixture::PackedUnalignedInteger => 8,
                CorpusFixture::CrossingBitfield => 9,
                CorpusFixture::NestedUnionAndDouble => 10,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(structural_families.len(), 11);

        let plan_inputs = corpus
            .iter()
            .map(|case| (case.fixture, case.allocation))
            .collect::<BTreeSet<_>>();
        assert_eq!(plan_inputs.len(), 4096);
        for observed in [
            corpus
                .iter()
                .map(|case| case.allocation.leading_integer)
                .collect::<BTreeSet<_>>(),
            corpus
                .iter()
                .map(|case| case.allocation.trailing_integer)
                .collect::<BTreeSet<_>>(),
        ] {
            assert_eq!(observed, (0..=6).collect());
        }
        for observed in [
            corpus
                .iter()
                .map(|case| case.allocation.leading_sse)
                .collect::<BTreeSet<_>>(),
            corpus
                .iter()
                .map(|case| case.allocation.trailing_sse)
                .collect::<BTreeSet<_>>(),
        ] {
            assert_eq!(observed, (0..=8).collect());
        }
    }

    #[test]
    fn selected_ids_have_a_stable_digest() {
        let mut hasher = Sha256::new();
        for case in selected_cross_link_cases() {
            hasher.update(case.id.as_bytes());
            hasher.update([0]);
        }
        assert_eq!(
            hex(&hasher.finalize()),
            "7b6301063f49d36aa392a7cf5680604f01c3e7382dbcc7163201848102ec58e2"
        );
    }
}
