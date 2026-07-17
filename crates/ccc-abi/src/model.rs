use std::collections::BTreeMap;
use std::fmt;

use ccc_ir::{InstructionId, ValueId};
use ccc_sema::generic::FullFunctionId;
use ccc_session::Span;
use ccc_target::CallingConvention;
use ccc_types::TypeId;

/// The source-level scalar representation carried across a native boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AbiScalar {
    SignedInteger { bits: u8 },
    UnsignedInteger { bits: u8 },
    Pointer { bits: u8 },
    Float32,
    Float64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AbiClass {
    NoClass,
    Integer,
    Sse,
    SseUp,
    X87,
    X87Up,
    ComplexX87,
    Memory,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AbiCarrier {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IntegerExtension {
    None,
    Signed,
    Unsigned,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NativePurpose {
    Normal,
    StructArgument(u32),
    StructReturn,
}

/// One Cranelift signature carrier. This deliberately does not contain a
/// machine register or stack offset: Cranelift is the placement authority for
/// native boundaries.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeCarrierPlan {
    pub abi_param_index: u32,
    pub source_index: Option<u32>,
    pub piece_index: Option<u8>,
    pub source_offset: u64,
    pub valid_bytes: u8,
    pub class: AbiClass,
    pub carrier: AbiCarrier,
    pub extension: IntegerExtension,
    pub purpose: NativePurpose,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AbiPiece {
    pub index: u8,
    pub offset: u64,
    pub valid_bytes: u8,
    pub class: AbiClass,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ClassifiedType {
    pub ty: TypeId,
    pub size: u64,
    pub align: u64,
    pub classes: Vec<AbiClass>,
    pub pieces: Vec<AbiPiece>,
    pub passing: PassingMode,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PassingMode {
    Void,
    Scalar,
    Registers,
    Memory,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeParameterPlan {
    pub source_index: u32,
    pub ty: TypeId,
    pub classified: ClassifiedType,
    pub carrier_indices: Vec<u32>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum NativeResultPlan {
    Void,
    Scalar {
        ty: TypeId,
        carrier_index: u32,
    },
    RegisterAggregate {
        classified: ClassifiedType,
        carrier_indices: Vec<u32>,
    },
    Indirect {
        classified: ClassifiedType,
        sret_parameter_index: u32,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeBoundaryPlan {
    pub calling_convention: CallingConvention,
    pub parameters: Vec<NativeParameterPlan>,
    pub result: NativeResultPlan,
    pub clif_parameters: Vec<NativeCarrierPlan>,
    pub clif_results: Vec<NativeCarrierPlan>,
    pub variadic: bool,
}

/// Compatibility alias for callers that plan a single nonvariadic function.
pub type FunctionPlan = NativeBoundaryPlan;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GpRegister {
    Rax,
    Rdi,
    Rsi,
    Rdx,
    Rcx,
    R8,
    R9,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SseRegister {
    Xmm0,
    Xmm1,
    Xmm2,
    Xmm3,
    Xmm4,
    Xmm5,
    Xmm6,
    Xmm7,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BridgeLocation {
    Gp(GpRegister),
    Sse(SseRegister),
    Stack { offset: u32 },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BridgePiecePlan {
    pub source_index: Option<u32>,
    pub piece: AbiPiece,
    pub extension: IntegerExtension,
    pub location: BridgeLocation,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BridgeKind {
    UnprototypedCall,
    VariadicCall,
    VariadicEntry,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BridgeBoundaryPlan {
    pub calling_convention: CallingConvention,
    pub kind: BridgeKind,
    pub parameters: Vec<ClassifiedType>,
    pub parameter_pieces: Vec<BridgePiecePlan>,
    pub result: ClassifiedType,
    pub result_pieces: Vec<BridgePiecePlan>,
    pub hidden_return: bool,
    /// Byte offset of the first unnamed stack argument from the start of the
    /// incoming stack argument area. This excludes trailing call alignment.
    pub overflow_arg_offset: u32,
    pub stack_size: u32,
    pub gp_used: u8,
    pub xmm_used: u8,
    pub variadic_sse_count: u8,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum BoundaryPlan {
    Native(NativeBoundaryPlan),
    Bridge(BridgeBoundaryPlan),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum LoweredSignaturePlan {
    Native {
        parameters: Vec<NativeCarrierPlan>,
        results: Vec<NativeCarrierPlan>,
    },
    UniformFramePointer,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CallTarget {
    Direct(FullFunctionId),
    Indirect(ValueId),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DefinitionPlan {
    pub source_signature: TypeId,
    pub lowered_signature: LoweredSignaturePlan,
    pub source_location: Span,
    pub boundary: BoundaryPlan,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CallPlan {
    pub source_signature: TypeId,
    pub lowered_signature: LoweredSignaturePlan,
    pub target: CallTarget,
    pub promoted_actual_types: Vec<TypeId>,
    pub fixed_boundary: usize,
    pub source_location: Span,
    pub boundary: BoundaryPlan,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SourceLinkage {
    None,
    Internal,
    External,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SourceVisibility {
    Default,
    Hidden,
    Protected,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallBridgeArtifactPlan {
    /// One uniform helper serves every listed call site.
    pub helper_symbol: String,
    pub call_sites: Vec<(FullFunctionId, InstructionId)>,
    pub frame_version: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariadicEntryArtifactPlan {
    pub function: FullFunctionId,
    pub public_symbol: String,
    pub source_linkage: SourceLinkage,
    pub source_visibility: SourceVisibility,
    pub body_symbol: String,
    pub frame_version: u16,
    pub va_state_version: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackagingPlan {
    pub generated_assembly_units: u32,
    pub requires_assembler: bool,
    pub requires_relocatable_link: bool,
    pub requires_object_copier: bool,
    /// Collision-proof generated and source-internal symbols localized after
    /// the relocatable link. User hidden symbols never enter this allowlist.
    pub exact_localization_symbols: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeArtifactPlan {
    pub call_bridge: Option<CallBridgeArtifactPlan>,
    pub variadic_entries: BTreeMap<FullFunctionId, VariadicEntryArtifactPlan>,
    pub packaging: PackagingPlan,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AbiConfigKey {
    pub schema: &'static str,
    pub target_triple: String,
    pub data_layout: String,
    pub calling_convention: CallingConvention,
    pub boundary_profile: &'static str,
    pub classifier_revision: u32,
    pub psabi_commit: &'static str,
    pub psabi_source_sha256: &'static str,
    pub backend_profile: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct IrShapeDigest(pub [u8; 32]);

impl IrShapeDigest {
    pub fn to_hex(self) -> String {
        hex(&self.0)
    }
}

impl fmt::Display for IrShapeDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex(&self.0))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TranslationUnitDigest(pub [u8; 32]);

impl TranslationUnitDigest {
    pub fn to_hex(self) -> String {
        hex(&self.0)
    }
}

impl fmt::Display for TranslationUnitDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex(&self.0))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleAbiPlan {
    pub config_key: AbiConfigKey,
    pub ir_shape_digest: IrShapeDigest,
    pub translation_unit_digest: TranslationUnitDigest,
    pub definitions: BTreeMap<FullFunctionId, DefinitionPlan>,
    pub calls: BTreeMap<(FullFunctionId, InstructionId), CallPlan>,
    pub va_args: BTreeMap<(FullFunctionId, InstructionId), VaArgPlan>,
    pub artifacts: BridgeArtifactPlan,
}

/// Proof that a module plan still describes the exact IR and ABI
/// configuration supplied to code generation.
#[derive(Clone, Copy, Debug)]
pub struct VerifiedModuleAbiPlan<'a> {
    pub(crate) plan: &'a ModuleAbiPlan,
}

impl<'a> VerifiedModuleAbiPlan<'a> {
    pub const fn plan(self) -> &'a ModuleAbiPlan {
        self.plan
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct VaArgPlan {
    pub classified: ClassifiedType,
    pub gp_slots: u8,
    pub sse_slots: u8,
    pub result_size: u64,
    pub result_align: u64,
    pub overflow_size: u64,
    pub overflow_align: u64,
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    output
}
