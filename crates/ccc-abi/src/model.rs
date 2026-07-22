use std::collections::BTreeMap;
use std::fmt;

use ccc_ir::{DataId, InstructionId, ValueId};
use ccc_sema::generic::{FullFunctionId, TlsModel};
use ccc_session::Span;
use ccc_target::{AbiIdentity, CallingConvention};
use ccc_types::TypeId;

/// The source-level scalar representation carried across a native boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AbiScalar {
    SignedInteger { bits: u8 },
    UnsignedInteger { bits: u8 },
    Pointer { bits: u8 },
    Float16,
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
    I128,
    F16,
    F32,
    F64,
    V32,
    V64,
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
    /// An ABI-mandated pointer to a caller-owned aggregate copy.
    IndirectArgument,
    StructReturn,
    /// A register-position hole required by an aggregate alignment rule.
    Padding,
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
pub enum RegisterBank {
    Integer,
    Float,
    /// The x87 return stack. Generated System V AMD64 bridges are the only
    /// consumers; this is never exposed as a Cranelift carrier.
    X87,
}

/// A register slot in an ABI-defined argument or result bank.
///
/// Machine register spellings deliberately stay in the assembly renderer;
/// ABI plans use only a bank and ordinal so the same model covers AMD64,
/// arm64, and RISC-V.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegisterSlot {
    pub bank: RegisterBank,
    pub index: u8,
}

impl RegisterSlot {
    pub const fn integer(index: u8) -> Self {
        Self {
            bank: RegisterBank::Integer,
            index,
        }
    }

    pub const fn float(index: u8) -> Self {
        Self {
            bank: RegisterBank::Float,
            index,
        }
    }

    pub const fn x87() -> Self {
        Self {
            bank: RegisterBank::X87,
            index: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BridgeLocation {
    Register(RegisterSlot),
    Stack { offset: u32 },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BridgePiecePlan {
    pub source_index: Option<u32>,
    pub piece: AbiPiece,
    pub extension: IntegerExtension,
    /// The physical piece carries a pointer to a caller-owned aggregate copy
    /// rather than bytes from the aggregate itself.
    pub indirect: bool,
    pub location: BridgeLocation,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BridgeKind {
    UnprototypedCall,
    VariadicCall,
    VariadicEntry,
    FixedCall,
    FixedEntry,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BridgeBoundaryPlan {
    pub abi_identity: AbiIdentity,
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SourceBinding {
    Strong,
    Weak,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallBridgeArtifactPlan {
    /// One uniform helper serves every listed call site.
    pub helper_symbol: String,
    pub call_sites: Vec<(FullFunctionId, InstructionId)>,
    pub frame_version: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeEntryArtifactPlan {
    pub function: FullFunctionId,
    pub kind: BridgeKind,
    pub public_symbol: String,
    pub public_symbol_is_exact: bool,
    pub source_linkage: SourceLinkage,
    pub source_visibility: SourceVisibility,
    pub source_binding: SourceBinding,
    pub body_symbol: String,
    pub frame_version: u16,
    pub va_state_version: u16,
}

/// One compiler-generated function that materializes the address of a TLS
/// object without relying on Cranelift's target-specific `tls_value` lowering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsAccessorArtifactPlan {
    pub object: DataId,
    pub object_symbol: String,
    pub helper_symbol: String,
    pub model: TlsModel,
    pub source_linkage: SourceLinkage,
    pub source_visibility: SourceVisibility,
    pub source_defined: bool,
}

/// One translation-unit-local x87 operation dispatcher. Its only public ABI
/// is `void(frame*)`; all f80 values remain address-backed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct F80SupportArtifactPlan {
    pub helper_symbol: String,
}

/// Translation-unit-local helper functions for the closed x86 instruction
/// forms retained in generic IR.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineAsmSupportArtifactPlan {
    pub cpuid_symbol: Option<String>,
    pub rdtsc_symbol: Option<String>,
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
    pub bridge_entries: BTreeMap<FullFunctionId, BridgeEntryArtifactPlan>,
    pub tls_accessors: BTreeMap<DataId, TlsAccessorArtifactPlan>,
    pub f80_support: Option<F80SupportArtifactPlan>,
    pub inline_asm_support: Option<InlineAsmSupportArtifactPlan>,
    pub packaging: PackagingPlan,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AbiConfigKey {
    pub schema: &'static str,
    pub target_triple: String,
    pub abi_identity: AbiIdentity,
    pub data_layout: String,
    pub calling_convention: CallingConvention,
    pub boundary_profile: &'static str,
    pub classifier_revision: u32,
    pub specification_revision: &'static str,
    pub specification_source_sha256: &'static str,
    pub backend_profile: &'static str,
    pub normalized_target_arch: &'static str,
    pub normalized_target_abi: &'static str,
    pub normalized_target_cpu: &'static str,
    pub normalized_deployment_target: String,
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
    /// The argument slot contains a pointer to the requested object.
    pub indirect: bool,
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
