//! Stable generated-assembly protocols used at SysV AMD64 boundaries.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::mem::offset_of;

use sha2::{Digest as _, Sha256};

use ccc_target::AbiIdentity;

use crate::{LinkError, artifact_error};

/// Magic identifying a generated call frame in diagnostic output.
pub const BRIDGE_FRAME_MAGIC: u32 = 0x4642_4343;
/// Magic identifying compiler-owned variadic state in diagnostic output.
pub const VA_STATE_MAGIC: u32 = 0x4156_4343;
/// The call target returns an x87 value which must be captured into the frame.
pub const BRIDGE_FLAG_X87_RESULT: u8 = 1;

/// The fixed portion of the call-helper protocol.
///
/// The trailing outgoing stack payload starts immediately after this value.
/// Producers initialize the header, live register prefixes, and complete
/// padded stack payload. Assembly helpers use the header high-water counts to
/// avoid reading inactive register slots.
#[repr(C, align(16))]
#[derive(Clone)]
pub struct BridgeFrameV2 {
    pub magic: u32,
    pub version: u16,
    pub header_size: u16,
    pub target_address: u64,
    pub outgoing_stack_size: u32,
    pub total_size: u32,
    pub gp_used: u8,
    pub xmm_used: u8,
    pub variadic_sse_count: u8,
    pub gp_results: u8,
    pub xmm_results: u8,
    pub flags: u8,
    pub reserved: u16,
    pub gp_slots: [[u8; 8]; 6],
    pub xmm_slots: [[u8; 16]; 8],
    pub gp_result_slots: [[u8; 8]; 2],
    pub xmm_result_slots: [[u8; 16]; 2],
    pub x87_result_slot: [u8; 16],
}

impl BridgeFrameV2 {
    pub const VERSION: u16 = 2;
    pub const HEADER_SIZE: u16 = 32;
    pub const FIXED_SIZE: usize = 272;
    pub const ALIGNMENT: usize = 16;

    /// Creates a fully zeroed Rust-side frame. Code generation may initialize
    /// only the protocol fields and live slots consumed by an assembly helper.
    pub fn zeroed(target_address: u64, outgoing_stack_size: u32) -> Self {
        let total_size = u32::try_from(Self::FIXED_SIZE)
            .expect("bridge frame size fits u32")
            .checked_add(outgoing_stack_size)
            .expect("bridge frame allocation fits u32");
        Self {
            magic: BRIDGE_FRAME_MAGIC,
            version: Self::VERSION,
            header_size: Self::HEADER_SIZE,
            target_address,
            outgoing_stack_size,
            total_size,
            gp_used: 0,
            xmm_used: 0,
            variadic_sse_count: 0,
            gp_results: 0,
            xmm_results: 0,
            flags: 0,
            reserved: 0,
            gp_slots: [[0; 8]; 6],
            xmm_slots: [[0; 16]; 8],
            gp_result_slots: [[0; 8]; 2],
            xmm_result_slots: [[0; 16]; 2],
            x87_result_slot: [0; 16],
        }
    }

    pub fn set_variadic_sse_count(&mut self, actual_sse_registers: u8) {
        self.variadic_sse_count = actual_sse_registers.min(8);
    }
}

/// Compiler-owned storage whose public `va_list` view starts at `gp_offset`.
#[repr(C)]
#[derive(Clone)]
pub struct VaStateV1 {
    pub magic: u32,
    pub version: u16,
    pub size: u16,
    pub gp_offset: u32,
    pub fp_offset: u32,
    pub overflow_arg_area: u64,
    pub reg_save_area: u64,
    pub register_save_area: [u8; 176],
}

impl VaStateV1 {
    pub const VERSION: u16 = 1;
    pub const SIZE: usize = 208;
    pub const VA_LIST_OFFSET: usize = 8;
    pub const REGISTER_SAVE_AREA_OFFSET: usize = 32;

    pub fn zeroed() -> Self {
        Self {
            magic: VA_STATE_MAGIC,
            version: Self::VERSION,
            size: u16::try_from(Self::SIZE).expect("variadic state size fits u16"),
            gp_offset: 0,
            fp_offset: 48,
            overflow_arg_area: 0,
            reg_save_area: 0,
            register_save_area: [0; 176],
        }
    }
}

/// Uniform frame passed by a public variadic assembly entry to its hidden
/// nonvariadic CLIF body.
#[repr(C, align(16))]
#[derive(Clone)]
pub struct VariadicEntryFrameV2 {
    pub va_state: VaStateV1,
    pub gp_result_slots: [[u8; 8]; 2],
    pub xmm_result_slots: [[u8; 16]; 2],
    pub x87_result_slot: [u8; 16],
}

impl VariadicEntryFrameV2 {
    pub const SIZE: usize = 272;
    pub const GP_RESULTS_OFFSET: usize = 208;
    pub const XMM_RESULTS_OFFSET: usize = 224;
    pub const X87_RESULT_OFFSET: usize = 256;

    pub fn zeroed() -> Self {
        Self {
            va_state: VaStateV1::zeroed(),
            gp_result_slots: [[0; 8]; 2],
            xmm_result_slots: [[0; 16]; 2],
            x87_result_slot: [0; 16],
        }
    }
}

const _: () = {
    assert!(size_of::<BridgeFrameV2>() == BridgeFrameV2::FIXED_SIZE);
    assert!(align_of::<BridgeFrameV2>() == BridgeFrameV2::ALIGNMENT);
    assert!(offset_of!(BridgeFrameV2, target_address) == 8);
    assert!(offset_of!(BridgeFrameV2, gp_slots) == 32);
    assert!(offset_of!(BridgeFrameV2, xmm_slots) == 80);
    assert!(offset_of!(BridgeFrameV2, gp_result_slots) == 208);
    assert!(offset_of!(BridgeFrameV2, xmm_result_slots) == 224);
    assert!(offset_of!(BridgeFrameV2, x87_result_slot) == 256);
    assert!(size_of::<VaStateV1>() == VaStateV1::SIZE);
    assert!(offset_of!(VaStateV1, gp_offset) == VaStateV1::VA_LIST_OFFSET);
    assert!(offset_of!(VaStateV1, register_save_area) == 32);
    assert!(size_of::<VariadicEntryFrameV2>() == VariadicEntryFrameV2::SIZE);
    assert!(offset_of!(VariadicEntryFrameV2, gp_result_slots) == 208);
    assert!(offset_of!(VariadicEntryFrameV2, xmm_result_slots) == 224);
    assert!(offset_of!(VariadicEntryFrameV2, x87_result_slot) == 256);
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GeneratedSymbolKind {
    CallHelper,
    CallStub,
    TlsAccessor,
    TlsObject,
    VariadicEntry,
    VariadicBody,
    Support,
    FixedEntry,
    FixedBody,
}

impl GeneratedSymbolKind {
    fn tag(self) -> &'static str {
        match self {
            Self::CallHelper => "call-helper",
            Self::CallStub => "call-stub",
            Self::TlsAccessor => "tls-accessor",
            Self::TlsObject => "tls-object",
            Self::VariadicEntry => "variadic-entry",
            Self::VariadicBody => "variadic-body",
            Self::Support => "support",
            Self::FixedEntry => "fixed-entry",
            Self::FixedBody => "fixed-body",
        }
    }

    fn identifier_component(self) -> &'static str {
        match self {
            Self::CallHelper => "call_helper",
            Self::CallStub => "call_stub",
            Self::TlsAccessor => "tls_accessor",
            Self::TlsObject => "tls_object",
            Self::VariadicEntry => "variadic_entry",
            Self::VariadicBody => "variadic_body",
            Self::Support => "support",
            Self::FixedEntry => "fixed_entry",
            Self::FixedBody => "fixed_body",
        }
    }
}

/// Derives a collision-resistant internal symbol from canonical inputs.
pub fn generated_symbol_name(
    translation_unit_digest: &[u8; 32],
    kind: GeneratedSymbolKind,
    stable_identity: &[u8],
    canonical_plan: &[u8],
) -> String {
    let mut digest = Sha256::new();
    hash_field(&mut digest, b"ccc-generated-symbol-v1");
    hash_field(&mut digest, translation_unit_digest);
    hash_field(&mut digest, kind.tag().as_bytes());
    hash_field(&mut digest, stable_identity);
    hash_field(&mut digest, canonical_plan);
    format!(
        "__ccc_{}_{:x}",
        kind.identifier_component(),
        digest.finalize()
    )
}

fn hash_field(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    digest.update(value);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalDebugLocation {
    pub logical_file: String,
    pub line: u32,
}

impl LogicalDebugLocation {
    pub fn generated(line: u32) -> Self {
        Self {
            logical_file: "<ccc-generated-bridge>".to_owned(),
            line,
        }
    }
}

/// One deterministic assembly input owned by an artifact bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedAssembly {
    stem: String,
    source: String,
    defined_symbols: Vec<String>,
    debug_locations: Vec<LogicalDebugLocation>,
}

impl GeneratedAssembly {
    pub fn new(
        stem: impl Into<String>,
        source: impl Into<String>,
        defined_symbols: Vec<String>,
        debug_locations: Vec<LogicalDebugLocation>,
    ) -> Result<Self, LinkError> {
        let stem = stem.into();
        if !is_safe_stem(&stem) {
            return Err(artifact_error(format!(
                "generated assembly stem `{stem}` is not a portable file stem"
            )));
        }
        let source = source.into();
        if source.as_bytes().contains(&0) {
            return Err(artifact_error(format!(
                "generated assembly `{stem}` contains a NUL byte"
            )));
        }
        let mut seen = BTreeSet::new();
        for symbol in &defined_symbols {
            validate_symbol(symbol)?;
            if !seen.insert(symbol.clone()) {
                return Err(artifact_error(format!(
                    "generated assembly `{stem}` declares `{symbol}` more than once"
                )));
            }
        }
        Ok(Self {
            stem,
            source,
            defined_symbols,
            debug_locations,
        })
    }

    pub fn stem(&self) -> &str {
        &self.stem
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn defined_symbols(&self) -> &[String] {
        &self.defined_symbols
    }

    pub fn debug_locations(&self) -> &[LogicalDebugLocation] {
        &self.debug_locations
    }
}

fn is_safe_stem(stem: &str) -> bool {
    !stem.is_empty()
        && stem.len() <= 96
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(crate) fn validate_symbol(symbol: &str) -> Result<(), LinkError> {
    let mut bytes = symbol.bytes();
    let Some(first) = bytes.next() else {
        return Err(artifact_error("generated symbol name is empty"));
    };
    if !(first.is_ascii_alphabetic() || matches!(first, b'_' | b'.'))
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$'))
    {
        return Err(artifact_error(format!(
            "`{symbol}` is not a supported ELF symbol spelling"
        )));
    }
    Ok(())
}

pub(crate) fn is_bridge_generated_symbol(symbol: &str) -> bool {
    [
        "__ccc_call_helper_",
        "__ccc_call_stub_",
        "__ccc_tls_accessor_",
        "__ccc_tls_object_",
        "__ccc_variadic_entry_",
        "__ccc_variadic_body_",
        "__ccc_fixed_entry_",
        "__ccc_fixed_body_",
        "__ccc_support_",
    ]
    .iter()
    .any(|prefix| symbol.starts_with(prefix))
}

/// Target TLS address sequence selected by the source-level TLS model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlsAccessModel {
    GeneralDynamic,
    LocalDynamic,
    InitialExec,
    LocalExec,
}

/// Visibility that the generated accessor must attach to its TLS reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlsSymbolVisibility {
    Default,
    Hidden,
    Protected,
    Internal,
}

/// Canonical inputs for one compiler-generated TLS address accessor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsAccessorPlan {
    pub abi: AbiIdentity,
    pub helper_symbol: String,
    pub object_symbol: String,
    /// Whether the object spelling is already the physical object-file name.
    pub object_symbol_is_exact: bool,
    pub model: TlsAccessModel,
    pub object_visibility: TlsSymbolVisibility,
    pub logical_line: u32,
}

impl TlsAccessorPlan {
    fn validate(&self) -> Result<(), LinkError> {
        validate_symbol(&self.helper_symbol)?;
        validate_symbol(&self.object_symbol)?;
        if !self.helper_symbol.starts_with("__ccc_tls_accessor_") {
            return Err(artifact_error(
                "a TLS accessor helper must use the reserved compiler namespace",
            ));
        }
        if self.helper_symbol == self.object_symbol {
            return Err(artifact_error(
                "a TLS accessor and its source object must use distinct symbols",
            ));
        }
        Ok(())
    }
}

/// Renders a hidden function that returns the calling thread's address for one
/// TLS object. Resolver calls preserve the target ABI's stack and unwind
/// contracts, and every Linux sequence uses linker-relaxable relocations.
pub fn render_target_tls_accessor(plan: &TlsAccessorPlan) -> Result<GeneratedAssembly, LinkError> {
    plan.validate()?;
    let source = match plan.abi {
        AbiIdentity::SysvAmd64Lp64 => render_x86_64_tls_accessor(plan),
        AbiIdentity::Aapcs64Lp64 => render_aarch64_tls_accessor(plan),
        AbiIdentity::RiscvLp64d => render_riscv64_tls_accessor(plan),
        AbiIdentity::DarwinArm64 => render_darwin_arm64_tls_accessor(plan),
    };
    GeneratedAssembly::new(
        format!("tls-accessor-{}", plan.helper_symbol),
        source,
        vec![plan.helper_symbol.clone()],
        vec![LogicalDebugLocation::generated(plan.logical_line.max(1))],
    )
}

fn render_elf_tls_visibility(symbol: &str, visibility: TlsSymbolVisibility, source: &mut String) {
    match visibility {
        TlsSymbolVisibility::Default => {}
        TlsSymbolVisibility::Hidden => {
            writeln!(source, ".hidden {symbol}").unwrap();
        }
        TlsSymbolVisibility::Protected => {
            writeln!(source, ".protected {symbol}").unwrap();
        }
        TlsSymbolVisibility::Internal => {
            writeln!(source, ".internal {symbol}").unwrap();
        }
    }
}

fn render_x86_64_tls_accessor(plan: &TlsAccessorPlan) -> String {
    let mut source = String::new();
    assembly_prelude(&mut source);
    render_elf_tls_visibility(&plan.object_symbol, plan.object_visibility, &mut source);
    function_header(
        &plan.helper_symbol,
        AssemblyFunctionLinkage::ExternalHidden,
        false,
        &mut source,
    );
    match plan.model {
        TlsAccessModel::GeneralDynamic => {
            source.push_str("subq $8, %rsp\n.cfi_def_cfa_offset 16\n");
            writeln!(
                source,
                "data16 leaq {}@TLSGD(%rip), %rdi",
                plan.object_symbol
            )
            .unwrap();
            source.push_str(".value 0x6666\nrex64\ncall __tls_get_addr@PLT\n");
            source.push_str("addq $8, %rsp\n.cfi_def_cfa_offset 8\nret\n");
        }
        TlsAccessModel::LocalDynamic => {
            source.push_str("subq $8, %rsp\n.cfi_def_cfa_offset 16\n");
            writeln!(source, "leaq {}@TLSLD(%rip), %rdi", plan.object_symbol).unwrap();
            source.push_str("call __tls_get_addr@PLT\n");
            writeln!(source, "leaq {}@DTPOFF(%rax), %rax", plan.object_symbol).unwrap();
            source.push_str("addq $8, %rsp\n.cfi_def_cfa_offset 8\nret\n");
        }
        TlsAccessModel::InitialExec => {
            source.push_str("movq %fs:0, %rax\n");
            writeln!(source, "addq {}@GOTTPOFF(%rip), %rax", plan.object_symbol).unwrap();
            source.push_str("ret\n");
        }
        TlsAccessModel::LocalExec => {
            source.push_str("movq %fs:0, %rax\n");
            writeln!(source, "leaq {}@TPOFF(%rax), %rax", plan.object_symbol).unwrap();
            source.push_str("ret\n");
        }
    }
    function_footer(&plan.helper_symbol, &mut source);
    source
}

fn render_aarch64_tls_accessor(plan: &TlsAccessorPlan) -> String {
    let mut source = String::new();
    render_elf_tls_visibility(&plan.object_symbol, plan.object_visibility, &mut source);
    target_function_header(
        &plan.helper_symbol,
        false,
        AssemblyFunctionLinkage::ExternalHidden,
        false,
        plan.abi,
        &mut source,
    );
    match plan.model {
        TlsAccessModel::GeneralDynamic | TlsAccessModel::LocalDynamic => {
            source.push_str(
                "stp x29, x30, [sp, #-16]!\n\
                 .cfi_def_cfa_offset 16\n\
                 mov x29, sp\n\
                 .cfi_def_cfa w29, 16\n\
                 .cfi_offset w30, -8\n\
                 .cfi_offset w29, -16\n",
            );
            writeln!(source, "adrp x0, :tlsdesc:{}", plan.object_symbol).unwrap();
            writeln!(source, "ldr x1, [x0, :tlsdesc_lo12:{}]", plan.object_symbol).unwrap();
            writeln!(source, "add x0, x0, :tlsdesc_lo12:{}", plan.object_symbol).unwrap();
            writeln!(source, ".tlsdesccall {}", plan.object_symbol).unwrap();
            source.push_str(
                "blr x1\n\
                 mrs x8, TPIDR_EL0\n\
                 add x0, x8, x0\n\
                 .cfi_def_cfa wsp, 16\n\
                 ldp x29, x30, [sp], #16\n\
                 .cfi_def_cfa_offset 0\n\
                 .cfi_restore w30\n\
                 .cfi_restore w29\n\
                 ret\n",
            );
        }
        TlsAccessModel::InitialExec => {
            writeln!(source, "adrp x9, :gottprel:{}", plan.object_symbol).unwrap();
            writeln!(
                source,
                "ldr x9, [x9, :gottprel_lo12:{}]",
                plan.object_symbol
            )
            .unwrap();
            source.push_str("mrs x8, TPIDR_EL0\nadd x0, x8, x9\nret\n");
        }
        TlsAccessModel::LocalExec => {
            source.push_str("mrs x8, TPIDR_EL0\n");
            writeln!(source, "add x8, x8, :tprel_hi12:{}", plan.object_symbol).unwrap();
            writeln!(source, "add x0, x8, :tprel_lo12_nc:{}", plan.object_symbol).unwrap();
            source.push_str("ret\n");
        }
    }
    target_function_footer(&plan.helper_symbol, false, plan.abi, &mut source);
    source
}

fn render_riscv64_tls_accessor(plan: &TlsAccessorPlan) -> String {
    let mut source = String::new();
    render_elf_tls_visibility(&plan.object_symbol, plan.object_visibility, &mut source);
    target_function_header(
        &plan.helper_symbol,
        false,
        AssemblyFunctionLinkage::ExternalHidden,
        false,
        plan.abi,
        &mut source,
    );
    match plan.model {
        TlsAccessModel::GeneralDynamic | TlsAccessModel::LocalDynamic => {
            source.push_str(
                "addi sp, sp, -16\n\
                 .cfi_def_cfa_offset 16\n\
                 sd ra, 8(sp)\n\
                 .cfi_offset ra, -8\n\
                 .Lccc_tls_dynamic:\n",
            );
            writeln!(source, "auipc a0, %tls_gd_pcrel_hi({})", plan.object_symbol).unwrap();
            source.push_str("addi a0, a0, %pcrel_lo(.Lccc_tls_dynamic)\ncall __tls_get_addr\n");
            source.push_str(
                "ld ra, 8(sp)\n\
                 .cfi_restore ra\n\
                 addi sp, sp, 16\n\
                 .cfi_def_cfa_offset 0\n\
                 ret\n",
            );
        }
        TlsAccessModel::InitialExec => {
            source.push_str(".Lccc_tls_initial_exec:\n");
            writeln!(source, "auipc a0, %tls_ie_pcrel_hi({})", plan.object_symbol).unwrap();
            source.push_str(
                "ld a0, %pcrel_lo(.Lccc_tls_initial_exec)(a0)\n\
                 add a0, a0, tp\n\
                 ret\n",
            );
        }
        TlsAccessModel::LocalExec => {
            writeln!(source, "lui a0, %tprel_hi({})", plan.object_symbol).unwrap();
            writeln!(source, "add a0, a0, tp, %tprel_add({})", plan.object_symbol).unwrap();
            writeln!(source, "addi a0, a0, %tprel_lo({})", plan.object_symbol).unwrap();
            source.push_str("ret\n");
        }
    }
    target_function_footer(&plan.helper_symbol, false, plan.abi, &mut source);
    source
}

fn render_darwin_arm64_tls_accessor(plan: &TlsAccessorPlan) -> String {
    let mut source = String::new();
    target_function_header(
        &plan.helper_symbol,
        false,
        AssemblyFunctionLinkage::ExternalHidden,
        false,
        plan.abi,
        &mut source,
    );
    let object_symbol =
        target_symbol_with_exactness(&plan.object_symbol, plan.abi, plan.object_symbol_is_exact);
    source.push_str(
        "stp x29, x30, [sp, #-16]!\n\
         .cfi_def_cfa_offset 16\n\
         mov x29, sp\n\
         .cfi_def_cfa w29, 16\n\
         .cfi_offset w30, -8\n\
         .cfi_offset w29, -16\n",
    );
    writeln!(source, "adrp x0, {object_symbol}@TLVPPAGE").unwrap();
    writeln!(source, "ldr x0, [x0, {object_symbol}@TLVPPAGEOFF]").unwrap();
    source.push_str(
        "ldr x8, [x0]\n\
         blr x8\n\
         .cfi_def_cfa wsp, 16\n\
         ldp x29, x30, [sp], #16\n\
         .cfi_def_cfa_offset 0\n\
         .cfi_restore w30\n\
         .cfi_restore w29\n\
         ret\n",
    );
    target_function_footer(&plan.helper_symbol, false, plan.abi, &mut source);
    source
}

fn assembly_prelude(output: &mut String) {
    output.push_str(".text\n");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssemblyFunctionLinkage {
    ExternalDefault,
    ExternalHidden,
    ExternalProtected,
    ExternalInternal,
    Internal,
}

fn function_header(
    symbol: &str,
    linkage: AssemblyFunctionLinkage,
    weak: bool,
    output: &mut String,
) {
    output.push_str(".p2align 4\n");
    let mut external_binding = || {
        writeln!(output, "{} {symbol}", if weak { ".weak" } else { ".globl" }).unwrap();
    };
    match linkage {
        AssemblyFunctionLinkage::ExternalDefault => {
            external_binding();
        }
        AssemblyFunctionLinkage::ExternalHidden => {
            external_binding();
            writeln!(output, ".hidden {symbol}").unwrap();
        }
        AssemblyFunctionLinkage::ExternalProtected => {
            external_binding();
            writeln!(output, ".protected {symbol}").unwrap();
        }
        AssemblyFunctionLinkage::ExternalInternal => {
            external_binding();
            writeln!(output, ".internal {symbol}").unwrap();
        }
        AssemblyFunctionLinkage::Internal => {
            // The primary object references this definition before the
            // partial link, so it must be linker-visible in its assembly
            // input. The verified manifest localizes it immediately after
            // resolution to restore source-level internal linkage.
            writeln!(output, ".globl {symbol}").unwrap();
            writeln!(output, ".hidden {symbol}").unwrap();
        }
    }
    writeln!(output, ".type {symbol}, @function").unwrap();
    writeln!(output, "{symbol}:").unwrap();
    output.push_str(".cfi_startproc\n");
}

fn function_footer(symbol: &str, output: &mut String) {
    output.push_str(".cfi_endproc\n");
    writeln!(output, ".size {symbol}, .-{symbol}").unwrap();
    output.push_str(".section .note.GNU-stack,\"\",@progbits\n");
}

fn target_symbol(symbol: &str, abi: AbiIdentity) -> String {
    target_symbol_with_exactness(symbol, abi, false)
}

fn target_symbol_with_exactness(symbol: &str, abi: AbiIdentity, exact: bool) -> String {
    if abi == AbiIdentity::DarwinArm64 && !exact {
        format!("_{symbol}")
    } else {
        symbol.to_owned()
    }
}

fn target_function_header(
    symbol: &str,
    symbol_is_exact: bool,
    linkage: AssemblyFunctionLinkage,
    weak: bool,
    abi: AbiIdentity,
    output: &mut String,
) {
    output.push_str(".text\n.p2align 2\n");
    let assembly_symbol = target_symbol_with_exactness(symbol, abi, symbol_is_exact);
    if abi == AbiIdentity::DarwinArm64 {
        if linkage != AssemblyFunctionLinkage::Internal {
            writeln!(output, ".globl {assembly_symbol}").unwrap();
        } else {
            // The primary Mach-O object references this definition, so it is
            // private external until the relocatable link resolves the pair.
            writeln!(output, ".globl {assembly_symbol}").unwrap();
        }
        if weak {
            writeln!(output, ".weak_definition {assembly_symbol}").unwrap();
        }
        if linkage != AssemblyFunctionLinkage::ExternalDefault {
            writeln!(output, ".private_extern {assembly_symbol}").unwrap();
        }
    } else {
        match linkage {
            AssemblyFunctionLinkage::ExternalDefault => {
                writeln!(
                    output,
                    "{} {assembly_symbol}",
                    if weak { ".weak" } else { ".globl" }
                )
                .unwrap();
            }
            AssemblyFunctionLinkage::ExternalHidden | AssemblyFunctionLinkage::Internal => {
                writeln!(
                    output,
                    "{} {assembly_symbol}",
                    if weak { ".weak" } else { ".globl" }
                )
                .unwrap();
                writeln!(output, ".hidden {assembly_symbol}").unwrap();
            }
            AssemblyFunctionLinkage::ExternalProtected => {
                writeln!(
                    output,
                    "{} {assembly_symbol}",
                    if weak { ".weak" } else { ".globl" }
                )
                .unwrap();
                writeln!(output, ".protected {assembly_symbol}").unwrap();
            }
            AssemblyFunctionLinkage::ExternalInternal => {
                writeln!(
                    output,
                    "{} {assembly_symbol}",
                    if weak { ".weak" } else { ".globl" }
                )
                .unwrap();
                writeln!(output, ".internal {assembly_symbol}").unwrap();
            }
        }
        writeln!(output, ".type {assembly_symbol}, @function").unwrap();
    }
    writeln!(output, "{assembly_symbol}:").unwrap();
    output.push_str(".cfi_startproc\n");
}

fn target_function_footer(
    symbol: &str,
    symbol_is_exact: bool,
    abi: AbiIdentity,
    output: &mut String,
) {
    output.push_str(".cfi_endproc\n");
    let assembly_symbol = target_symbol_with_exactness(symbol, abi, symbol_is_exact);
    if abi == AbiIdentity::DarwinArm64 {
        output.push_str(".subsections_via_symbols\n");
    } else {
        writeln!(output, ".size {assembly_symbol}, .-{assembly_symbol}").unwrap();
        output.push_str(".section .note.GNU-stack,\"\",@progbits\n");
    }
}

/// Renders the translation-unit call helper used by ABI bridge frames.
pub fn render_generic_call_helper(symbol: &str) -> Result<GeneratedAssembly, LinkError> {
    validate_symbol(symbol)?;
    let mut source = String::new();
    assembly_prelude(&mut source);
    function_header(
        symbol,
        AssemblyFunctionLinkage::ExternalHidden,
        false,
        &mut source,
    );
    source.push_str(
        "pushq %rbp\n\
         .cfi_def_cfa_offset 16\n\
         .cfi_offset %rbp, -16\n\
         movq %rsp, %rbp\n\
         .cfi_def_cfa_register %rbp\n\
         pushq %r12\n\
         .cfi_offset %r12, -24\n\
         pushq %r13\n\
         .cfi_offset %r13, -32\n\
         movq %rdi, %r12\n\
         movl 16(%r12), %r13d\n\
         addq $15, %r13\n\
         andq $-16, %r13\n\
         subq %r13, %rsp\n\
         movq %rsp, %rdi\n\
         leaq 272(%r12), %rsi\n\
         movl 16(%r12), %ecx\n\
         rep movsb\n\
         movq 8(%r12), %r11\n",
    );
    source.push_str("movzbl 25(%r12), %r10d\n");
    for index in 0..8 {
        writeln!(
            source,
            "cmpb ${index}, %r10b\n\
             jbe .Lccc_call_xmm_inputs_done\n\
             movdqu {}(%r12), %xmm{index}",
            80 + index * 16
        )
        .unwrap();
    }
    source.push_str(".Lccc_call_xmm_inputs_done:\nmovzbl 24(%r12), %r10d\n");
    for (index, register) in ["%rdi", "%rsi", "%rdx", "%rcx", "%r8", "%r9"]
        .into_iter()
        .enumerate()
    {
        writeln!(
            source,
            "cmpb ${index}, %r10b\n\
             jbe .Lccc_call_gp_inputs_done\n\
             movq {}(%r12), {register}",
            32 + index * 8
        )
        .unwrap();
    }
    source.push_str(
        ".Lccc_call_gp_inputs_done:\n\
         movzbl 26(%r12), %eax\n\
         call *%r11\n\
         movq %rax, 208(%r12)\n\
         movq %rdx, 216(%r12)\n\
         movdqu %xmm0, 224(%r12)\n\
         movdqu %xmm1, 240(%r12)\n\
         testb $1, 29(%r12)\n\
         jz .Lccc_call_no_x87_result\n\
         fstpt 256(%r12)\n\
         .Lccc_call_no_x87_result:\n\
         leaq -16(%rbp), %rsp\n\
         popq %r13\n\
         popq %r12\n\
         popq %rbp\n\
         .cfi_def_cfa %rsp, 8\n\
         ret\n",
    );
    function_footer(symbol, &mut source);
    GeneratedAssembly::new(
        "call-helper",
        source,
        vec![symbol.to_owned()],
        vec![LogicalDebugLocation::generated(1)],
    )
}

/// Renders the call-frame helper for the selected CCC ABI identity.
pub fn render_target_call_helper(
    symbol: &str,
    abi: AbiIdentity,
) -> Result<GeneratedAssembly, LinkError> {
    match abi {
        AbiIdentity::SysvAmd64Lp64 => render_generic_call_helper(symbol),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            render_arm64_call_helper(symbol, abi)
        }
        AbiIdentity::RiscvLp64d => render_riscv64_call_helper(symbol),
    }
}

/// Compiler-runtime conversions referenced by the x87 operation dispatcher.
///
/// Each flag controls both the accepted dispatcher opcode and the matching
/// undefined compiler-runtime symbol. This keeps helper selection
/// operation-sensitive even though all x87 operations share one generated
/// assembly unit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct F80RuntimeHelperPlan {
    pub from_i128: bool,
    pub from_u128: bool,
    pub to_i128: bool,
    pub to_u128: bool,
}

/// Renders the translation-unit-local x87 operation dispatcher.
///
/// The dispatcher accepts one pointer to a four-field frame containing an
/// opcode followed by output, left-input, and right-input pointers. Every
/// extended value is a pointer to a 16-byte object containing the 10-byte x87
/// payload. This keeps x87 registers entirely inside generated assembly.
pub fn render_f80_support(
    symbol: &str,
    runtime_helpers: F80RuntimeHelperPlan,
) -> Result<GeneratedAssembly, LinkError> {
    validate_symbol(symbol)?;
    let mut source = String::new();
    assembly_prelude(&mut source);
    function_header(
        symbol,
        AssemblyFunctionLinkage::ExternalHidden,
        false,
        &mut source,
    );
    source.push_str(
        "movl 0(%rdi), %eax\n\
         movq 8(%rdi), %r8\n\
         movq 16(%rdi), %r9\n\
         movq 24(%rdi), %r10\n\
         cmpl $1, %eax\n\
         je .Lccc_f80_add\n\
         cmpl $2, %eax\n\
         je .Lccc_f80_sub\n\
         cmpl $3, %eax\n\
         je .Lccc_f80_mul\n\
         cmpl $4, %eax\n\
         je .Lccc_f80_div\n\
         cmpl $5, %eax\n\
         je .Lccc_f80_cmp\n\
         cmpl $6, %eax\n\
         je .Lccc_f80_neg\n\
         cmpl $7, %eax\n\
         je .Lccc_f80_from_i64\n\
         cmpl $8, %eax\n\
         je .Lccc_f80_from_u64\n\
         cmpl $9, %eax\n\
         je .Lccc_f80_from_f32\n\
         cmpl $10, %eax\n\
         je .Lccc_f80_from_f64\n\
         cmpl $11, %eax\n\
         je .Lccc_f80_to_i64\n\
         cmpl $12, %eax\n\
         je .Lccc_f80_to_u64\n\
         cmpl $13, %eax\n\
         je .Lccc_f80_to_f32\n\
         cmpl $14, %eax\n\
         je .Lccc_f80_to_f64\n\
         cmpl $15, %eax\n\
         je .Lccc_f80_copy_x87\n\
         cmpl $16, %eax\n\
         je .Lccc_f80_copy_x87\n\
         cmpl $21, %eax\n\
         je .Lccc_f80_cmp_signaling\n\
",
    );
    if runtime_helpers.from_i128 {
        source.push_str("cmpl $17, %eax\nje .Lccc_f80_from_i128\n");
    }
    if runtime_helpers.from_u128 {
        source.push_str("cmpl $18, %eax\nje .Lccc_f80_from_u128\n");
    }
    if runtime_helpers.to_i128 {
        source.push_str("cmpl $19, %eax\nje .Lccc_f80_to_i128\n");
    }
    if runtime_helpers.to_u128 {
        source.push_str("cmpl $20, %eax\nje .Lccc_f80_to_u128\n");
    }
    source.push_str(
        "ud2\n\
         .Lccc_f80_add:\n\
         fldt (%r9)\n\
         fldt (%r10)\n\
         faddp %st, %st(1)\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_sub:\n\
         fldt (%r9)\n\
         fldt (%r10)\n\
         fsubrp %st, %st(1)\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_mul:\n\
         fldt (%r9)\n\
         fldt (%r10)\n\
         fmulp %st, %st(1)\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_div:\n\
         fldt (%r9)\n\
         fldt (%r10)\n\
         fdivrp %st, %st(1)\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_cmp:\n\
         fldt (%r10)\n\
         fldt (%r9)\n\
         fucomip %st(1), %st\n\
         fstp %st(0)\n\
         jmp .Lccc_f80_cmp_flags\n\
         .Lccc_f80_cmp_signaling:\n\
         fldt (%r10)\n\
         fldt (%r9)\n\
         fcomip %st(1), %st\n\
         fstp %st(0)\n\
         .Lccc_f80_cmp_flags:\n\
         jp .Lccc_f80_cmp_unordered\n\
         je .Lccc_f80_cmp_equal\n\
         jb .Lccc_f80_cmp_less\n\
         movl $1, (%r8)\n\
         ret\n\
         .Lccc_f80_cmp_less:\n\
         movl $-1, (%r8)\n\
         ret\n\
         .Lccc_f80_cmp_equal:\n\
         movl $0, (%r8)\n\
         ret\n\
         .Lccc_f80_cmp_unordered:\n\
         movl $2, (%r8)\n\
         ret\n\
         .Lccc_f80_neg:\n\
         movq 0(%r9), %rax\n\
         movq %rax, 0(%r8)\n\
         movzwl 8(%r9), %eax\n\
         xorl $32768, %eax\n\
         movw %ax, 8(%r8)\n\
         ret\n\
         .Lccc_f80_from_i64:\n\
         fildq (%r9)\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_from_u64:\n\
         movq (%r9), %rax\n\
         testq %rax, %rax\n\
         js .Lccc_f80_from_u64_high\n\
         fildq (%r9)\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_from_u64_high:\n\
         movq %rax, %rcx\n\
         andl $1, %ecx\n\
         shrq $1, %rax\n\
         movq %rax, -8(%rsp)\n\
         fildq -8(%rsp)\n\
         fildq -8(%rsp)\n\
         faddp %st, %st(1)\n\
         testl %ecx, %ecx\n\
         jz .Lccc_f80_from_u64_store\n\
         fld1\n\
         faddp %st, %st(1)\n\
         .Lccc_f80_from_u64_store:\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_from_f32:\n\
         flds (%r9)\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_from_f64:\n\
         fldl (%r9)\n\
         fstpt (%r8)\n\
         ret\n\
         .Lccc_f80_to_i64:\n\
         subq $16, %rsp\n\
         fnstcw 8(%rsp)\n\
         movzwl 8(%rsp), %eax\n\
         andl $62463, %eax\n\
         orl $3072, %eax\n\
         movw %ax, 10(%rsp)\n\
         fldcw 10(%rsp)\n\
         fldt (%r9)\n\
         fistpq (%r8)\n\
         fldcw 8(%rsp)\n\
         addq $16, %rsp\n\
         ret\n\
         .Lccc_f80_to_u64:\n\
         subq $16, %rsp\n\
         fnstcw 8(%rsp)\n\
         movzwl 8(%rsp), %eax\n\
         andl $62463, %eax\n\
         orl $3072, %eax\n\
         movw %ax, 10(%rsp)\n\
         fldcw 10(%rsp)\n\
         fldt (%r9)\n\
         fldt .Lccc_f80_two63(%rip)\n\
         fucomip %st(1), %st\n\
         jbe .Lccc_f80_to_u64_high\n\
         fistpq (%r8)\n\
         jmp .Lccc_f80_to_u64_done\n\
         .Lccc_f80_to_u64_high:\n\
         fldt .Lccc_f80_two63(%rip)\n\
         fsubrp %st, %st(1)\n\
         fistpq (%r8)\n\
         btcq $63, (%r8)\n\
         .Lccc_f80_to_u64_done:\n\
         fldcw 8(%rsp)\n\
         addq $16, %rsp\n\
         ret\n\
         .Lccc_f80_to_f32:\n\
         fldt (%r9)\n\
         fstps (%r8)\n\
         ret\n\
         .Lccc_f80_to_f64:\n\
         fldt (%r9)\n\
         fstpl (%r8)\n\
         ret\n\
         .Lccc_f80_copy_x87:\n\
         fldt (%r9)\n\
         fstpt (%r8)\n\
         ret\n\
",
    );
    if runtime_helpers.from_i128 {
        source.push_str(
            ".Lccc_f80_from_i128:\n\
             pushq %r12\n\
             .cfi_def_cfa_offset 16\n\
             .cfi_offset %r12, -16\n\
             movq %r8, %r12\n\
             movq 0(%r9), %rdi\n\
             movq 8(%r9), %rsi\n\
             call __floattixf@PLT\n\
             fstpt (%r12)\n\
             popq %r12\n\
             .cfi_def_cfa_offset 8\n\
             .cfi_restore %r12\n\
             ret\n",
        );
    }
    if runtime_helpers.from_u128 {
        source.push_str(
            ".Lccc_f80_from_u128:\n\
             pushq %r12\n\
             .cfi_def_cfa_offset 16\n\
             .cfi_offset %r12, -16\n\
             movq %r8, %r12\n\
             movq 0(%r9), %rdi\n\
             movq 8(%r9), %rsi\n\
             call __floatuntixf@PLT\n\
             fstpt (%r12)\n\
             popq %r12\n\
             .cfi_def_cfa_offset 8\n\
             .cfi_restore %r12\n\
             ret\n",
        );
    }
    if runtime_helpers.to_i128 {
        source.push_str(&render_f80_to_i128_helper(".Lccc_f80_to_i128", "__fixxfti"));
    }
    if runtime_helpers.to_u128 {
        source.push_str(&render_f80_to_i128_helper(
            ".Lccc_f80_to_u128",
            "__fixunsxfti",
        ));
    }
    source.push_str(
        ".pushsection .rodata\n\
         .p2align 4\n\
         .Lccc_f80_two63:\n\
         .quad 0x8000000000000000\n\
         .word 0x403e\n\
         .zero 6\n\
         .popsection\n",
    );
    function_footer(symbol, &mut source);
    GeneratedAssembly::new(
        "f80-support",
        source,
        vec![symbol.to_owned()],
        vec![LogicalDebugLocation::generated(1)],
    )
}

fn render_f80_to_i128_helper(label: &str, runtime_symbol: &str) -> String {
    format!(
        "{label}:\n\
         pushq %r12\n\
         .cfi_def_cfa_offset 16\n\
         .cfi_offset %r12, -16\n\
         movq %r8, %r12\n\
         subq $16, %rsp\n\
         .cfi_def_cfa_offset 32\n\
         movq 0(%r9), %rax\n\
         movq 8(%r9), %rdx\n\
         movq %rax, 0(%rsp)\n\
         movq %rdx, 8(%rsp)\n\
         call {runtime_symbol}@PLT\n\
         addq $16, %rsp\n\
         .cfi_def_cfa_offset 16\n\
         movq %rax, 0(%r12)\n\
         movq %rdx, 8(%r12)\n\
         popq %r12\n\
         .cfi_def_cfa_offset 8\n\
         .cfi_restore %r12\n\
         ret\n"
    )
}

/// Exact helper symbols selected by retained x86 inline-assembly operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineAsmSupportPlan {
    pub cpuid_symbol: Option<String>,
    pub rdtsc_symbol: Option<String>,
}

/// Renders one translation-unit-local support unit for CPUID and RDTSC.
///
/// CPUID accepts `(leaf, subleaf, eax*, ebx*, ecx*, edx*)` and stores every
/// requested output from one instruction execution. RDTSC accepts `(low*,
/// high*)` and likewise executes exactly once. Null CPUID outputs are allowed
/// so source forms can retain only selected registers.
pub fn render_inline_asm_support(
    plan: &InlineAsmSupportPlan,
) -> Result<GeneratedAssembly, LinkError> {
    if plan.cpuid_symbol.is_none() && plan.rdtsc_symbol.is_none() {
        return Err(artifact_error(
            "an inline assembly support unit must define at least one helper",
        ));
    }
    let mut source = String::new();
    let mut symbols = Vec::new();
    let mut locations = Vec::new();
    if let Some(symbol) = &plan.cpuid_symbol {
        validate_symbol(symbol)?;
        if !symbol.starts_with("__ccc_support_cpuid_") {
            return Err(artifact_error(
                "a CPUID helper must use the reserved compiler namespace",
            ));
        }
        assembly_prelude(&mut source);
        function_header(
            symbol,
            AssemblyFunctionLinkage::ExternalHidden,
            false,
            &mut source,
        );
        source.push_str(
            "pushq %rbx\n\
             .cfi_def_cfa_offset 16\n\
             .cfi_offset %rbx, -16\n\
             movq %rdx, %r10\n\
             movq %rcx, %r11\n\
             movl %edi, %eax\n\
             movl %esi, %ecx\n\
             cpuid\n\
             testq %r10, %r10\n\
             je 1f\n\
             movl %eax, (%r10)\n\
             1:\n\
             testq %r11, %r11\n\
             je 2f\n\
             movl %ebx, (%r11)\n\
             2:\n\
             testq %r8, %r8\n\
             je 3f\n\
             movl %ecx, (%r8)\n\
             3:\n\
             testq %r9, %r9\n\
             je 4f\n\
             movl %edx, (%r9)\n\
             4:\n\
             popq %rbx\n\
             .cfi_def_cfa_offset 8\n\
             .cfi_restore %rbx\n\
             ret\n",
        );
        function_footer(symbol, &mut source);
        symbols.push(symbol.clone());
        locations.push(LogicalDebugLocation::generated(1));
    }
    if let Some(symbol) = &plan.rdtsc_symbol {
        validate_symbol(symbol)?;
        if !symbol.starts_with("__ccc_support_rdtsc_") {
            return Err(artifact_error(
                "an RDTSC helper must use the reserved compiler namespace",
            ));
        }
        assembly_prelude(&mut source);
        function_header(
            symbol,
            AssemblyFunctionLinkage::ExternalHidden,
            false,
            &mut source,
        );
        source.push_str("rdtsc\nmovl %eax, (%rdi)\nmovl %edx, (%rsi)\nret\n");
        function_footer(symbol, &mut source);
        symbols.push(symbol.clone());
        locations.push(LogicalDebugLocation::generated(2));
    }
    GeneratedAssembly::new("inline-asm-support", source, symbols, locations)
}

fn render_arm64_call_helper(
    symbol: &str,
    abi: AbiIdentity,
) -> Result<GeneratedAssembly, LinkError> {
    validate_symbol(symbol)?;
    let mut source = String::new();
    target_function_header(
        symbol,
        false,
        AssemblyFunctionLinkage::ExternalHidden,
        false,
        abi,
        &mut source,
    );
    source.push_str(
        "stp x29, x30, [sp, #-32]!\n\
         .cfi_def_cfa_offset 32\n\
         .cfi_offset x29, -32\n\
         .cfi_offset x30, -24\n\
         stp x19, x20, [sp, #16]\n\
         .cfi_offset x19, -16\n\
         .cfi_offset x20, -8\n\
         mov x29, sp\n\
         .cfi_def_cfa_register x29\n\
         mov x19, x0\n\
         ldr w20, [x19, #16]\n\
         add x20, x20, #15\n\
         and x20, x20, #-16\n\
         sub sp, sp, x20\n\
         ldr w2, [x19, #16]\n\
         cbz x2, 2f\n\
         add x1, x19, #320\n\
         mov x0, sp\n\
         1:\n\
         ldrb w3, [x1], #1\n\
         strb w3, [x0], #1\n\
         subs x2, x2, #1\n\
         b.ne 1b\n\
         2:\n\
         ldr x16, [x19, #8]\n",
    );
    source.push_str(
        "ldrb w17, [x19, #25]\n\
         adr x9, 3f\n\
         sub x9, x9, x17, lsl #2\n\
         br x9\n",
    );
    for index in (0..8).rev() {
        writeln!(source, "ldr q{index}, [x19, #{}]", 112 + index * 16).unwrap();
    }
    source.push_str(
        "3:\n\
         ldrb w17, [x19, #24]\n\
         adr x9, 4f\n\
         sub x9, x9, x17, lsl #2\n\
         br x9\n",
    );
    for index in (0..8).rev() {
        writeln!(source, "ldr x{index}, [x19, #{}]", 48 + index * 8).unwrap();
    }
    source.push_str(
        "4:\n\
         ldr x8, [x19, #32]\n\
         blr x16\n\
         stp x0, x1, [x19, #240]\n\
         str q0, [x19, #256]\n\
         str q1, [x19, #272]\n\
         str q2, [x19, #288]\n\
         str q3, [x19, #304]\n\
         mov sp, x29\n\
         .cfi_def_cfa sp, 32\n\
         ldp x19, x20, [sp, #16]\n\
         .cfi_restore x19\n\
         .cfi_restore x20\n\
         ldp x29, x30, [sp], #32\n\
         .cfi_restore x29\n\
         .cfi_restore x30\n\
         .cfi_def_cfa sp, 0\n\
         ret\n",
    );
    target_function_footer(symbol, false, abi, &mut source);
    GeneratedAssembly::new(
        "call-helper",
        source,
        vec![symbol.to_owned()],
        vec![LogicalDebugLocation::generated(1)],
    )
}

fn render_riscv64_call_helper(symbol: &str) -> Result<GeneratedAssembly, LinkError> {
    validate_symbol(symbol)?;
    let mut source = String::new();
    target_function_header(
        symbol,
        false,
        AssemblyFunctionLinkage::ExternalHidden,
        false,
        AbiIdentity::RiscvLp64d,
        &mut source,
    );
    source.push_str(
        "addi sp, sp, -32\n\
         .cfi_def_cfa_offset 32\n\
         sd ra, 24(sp)\n\
         .cfi_offset ra, -8\n\
         sd s0, 16(sp)\n\
         .cfi_offset s0, -16\n\
         sd s1, 8(sp)\n\
         .cfi_offset s1, -24\n\
         sd s2, 0(sp)\n\
         .cfi_offset s2, -32\n\
         addi s0, sp, 32\n\
         .cfi_def_cfa s0, 0\n\
         mv s1, a0\n\
         lwu s2, 16(s1)\n\
         addi s2, s2, 15\n\
         andi s2, s2, -16\n\
         sub sp, sp, s2\n\
         lwu t2, 16(s1)\n\
         beqz t2, 2f\n\
         addi t0, s1, 320\n\
         mv t1, sp\n\
         1:\n\
         lbu t3, 0(t0)\n\
         sb t3, 0(t1)\n\
         addi t0, t0, 1\n\
         addi t1, t1, 1\n\
         addi t2, t2, -1\n\
         bnez t2, 1b\n\
         2:\n\
         ld t0, 8(s1)\n",
    );
    source.push_str("lbu t1, 25(s1)\nbeqz t1, .Lccc_call_riscv_fp_inputs_done\n");
    for index in 0..8 {
        writeln!(source, "fld fa{index}, {}(s1)", 112 + index * 16).unwrap();
        if index != 7 {
            source.push_str(
                "addi t1, t1, -1\n\
                 beqz t1, .Lccc_call_riscv_fp_inputs_done\n",
            );
        }
    }
    source.push_str(
        ".Lccc_call_riscv_fp_inputs_done:\n\
         lbu t1, 24(s1)\n\
         beqz t1, .Lccc_call_riscv_gp_inputs_done\n",
    );
    for index in 0..8 {
        writeln!(source, "ld a{index}, {}(s1)", 48 + index * 8).unwrap();
        if index != 7 {
            source.push_str(
                "addi t1, t1, -1\n\
                 beqz t1, .Lccc_call_riscv_gp_inputs_done\n",
            );
        }
    }
    source.push_str(
        ".Lccc_call_riscv_gp_inputs_done:\n\
         jalr t0\n\
         sd a0, 240(s1)\n\
         sd a1, 248(s1)\n\
         fsd fa0, 256(s1)\n\
         fsd fa1, 272(s1)\n\
         fsd fa2, 288(s1)\n\
         fsd fa3, 304(s1)\n\
         addi sp, s0, -32\n\
         .cfi_def_cfa sp, 32\n\
         ld s2, 0(sp)\n\
         .cfi_restore s2\n\
         ld s1, 8(sp)\n\
         .cfi_restore s1\n\
         ld s0, 16(sp)\n\
         .cfi_restore s0\n\
         ld ra, 24(sp)\n\
         .cfi_restore ra\n\
         addi sp, sp, 32\n\
         .cfi_def_cfa sp, 0\n\
         ret\n",
    );
    target_function_footer(symbol, false, AbiIdentity::RiscvLp64d, &mut source);
    GeneratedAssembly::new(
        "call-helper",
        source,
        vec![symbol.to_owned()],
        vec![LogicalDebugLocation::generated(1)],
    )
}

/// Precomputed information needed to render a public uniform-frame entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeEntryPlan {
    pub public_symbol: String,
    /// The public spelling is an explicit object-file symbol from a
    /// declaration assembly label and must bypass target C-name mangling.
    pub public_symbol_is_exact: bool,
    pub hidden_body_symbol: String,
    pub linkage: AssemblyFunctionLinkage,
    pub weak: bool,
    pub fixed_gp_used: u8,
    pub fixed_sse_used: u8,
    /// Offset from the incoming stack argument area to the first variadic item.
    pub overflow_arg_offset: u32,
    /// Number of integer result registers populated by the hidden body.
    pub gp_results: u8,
    /// Number of SSE result registers populated by the hidden body.
    pub xmm_results: u8,
    /// Restore one x87 long-double result from the hidden body frame.
    pub x87_result: bool,
    /// Echo the saved incoming structure-return pointer in `%rax`.
    pub hidden_return: bool,
    pub logical_line: u32,
}

impl BridgeEntryPlan {
    fn validate(&self, abi: AbiIdentity) -> Result<(), LinkError> {
        validate_symbol(&self.public_symbol)?;
        validate_symbol(&self.hidden_body_symbol)?;
        if self.public_symbol == self.hidden_body_symbol {
            return Err(artifact_error(
                "a bridge entry and its hidden body must use distinct symbols",
            ));
        }
        if self.weak && self.linkage == AssemblyFunctionLinkage::Internal {
            return Err(artifact_error(
                "an internal bridge entry cannot have weak binding",
            ));
        }
        let gp_limit = if abi == AbiIdentity::SysvAmd64Lp64 {
            6
        } else {
            8
        };
        if self.fixed_gp_used > gp_limit || self.fixed_sse_used > 8 {
            return Err(artifact_error(
                "bridge-entry register counts exceed the target register areas",
            ));
        }
        if !self.overflow_arg_offset.is_multiple_of(8) {
            return Err(artifact_error(
                "the bridge-entry overflow argument offset must be eight-byte aligned",
            ));
        }
        let float_result_limit =
            if matches!(abi, AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64) {
                4
            } else {
                2
            };
        if self.gp_results > 2 || self.xmm_results > float_result_limit {
            return Err(artifact_error(
                "a bridge-entry result exceeds the target register banks",
            ));
        }
        if self.x87_result
            && (abi != AbiIdentity::SysvAmd64Lp64 || self.gp_results != 0 || self.xmm_results != 0)
        {
            return Err(artifact_error(
                "an x87 bridge-entry result must be the sole System V AMD64 result",
            ));
        }
        if self.hidden_return && (self.gp_results != 0 || self.xmm_results != 0 || self.x87_result)
        {
            return Err(artifact_error(
                "an indirect bridge-entry result cannot also request register results",
            ));
        }
        Ok(())
    }
}

/// Renders the public assembly entry for a compiler-defined variadic function.
pub fn render_variadic_entry(plan: &BridgeEntryPlan) -> Result<GeneratedAssembly, LinkError> {
    render_target_variadic_entry(plan, AbiIdentity::SysvAmd64Lp64)
}

/// Renders a public variadic entry for the selected CCC ABI identity.
pub fn render_target_variadic_entry(
    plan: &BridgeEntryPlan,
    abi: AbiIdentity,
) -> Result<GeneratedAssembly, LinkError> {
    plan.validate(abi)?;

    match abi {
        AbiIdentity::SysvAmd64Lp64 => render_sysv_amd64_entry(plan, EntryKind::Variadic),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            render_arm64_variadic_entry(plan, abi)
        }
        AbiIdentity::RiscvLp64d => render_riscv64_variadic_entry(plan),
    }
}

/// Renders a public fixed-signature wide-integer entry for x86-64 System V.
///
/// Unlike a variadic entry, an ordinary call does not define `%al`. The fixed
/// renderer therefore saves every planned SSE input unconditionally.
pub fn render_target_fixed_entry(
    plan: &BridgeEntryPlan,
    abi: AbiIdentity,
) -> Result<GeneratedAssembly, LinkError> {
    plan.validate(abi)?;
    if abi != AbiIdentity::SysvAmd64Lp64 {
        return Err(artifact_error(
            "fixed uniform-frame entries are enabled only for x86-64 System V",
        ));
    }
    render_sysv_amd64_entry(plan, EntryKind::Fixed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    Fixed,
    Variadic,
}

fn render_sysv_amd64_entry(
    plan: &BridgeEntryPlan,
    kind: EntryKind,
) -> Result<GeneratedAssembly, LinkError> {
    let mut source = String::new();
    assembly_prelude(&mut source);
    function_header(&plan.public_symbol, plan.linkage, plan.weak, &mut source);
    source.push_str(
        "pushq %rbp\n\
         .cfi_def_cfa_offset 16\n\
         .cfi_offset %rbp, -16\n\
         movq %rsp, %rbp\n\
         .cfi_def_cfa_register %rbp\n",
    );
    // The public entry calls one uniform hidden signature: `fn(frame*) ->
    // void`. The body reconstructs fixed values from this frame, so assembly
    // never has to duplicate Cranelift's native aggregate signature.
    source.push_str("subq $272, %rsp\n");
    if kind == EntryKind::Variadic {
        // `%al` is caller-owned variadic metadata and must be captured before
        // any instruction reuses `%rax`.
        source.push_str("movzbl %al, %r11d\n");
    }
    for (register, offset) in [
        ("%rdi", 32),
        ("%rsi", 40),
        ("%rdx", 48),
        ("%rcx", 56),
        ("%r8", 64),
        ("%r9", 72),
    ] {
        writeln!(source, "movq {register}, {offset}(%rsp)").unwrap();
    }
    source.push_str("pxor %xmm15, %xmm15\n");
    for index in 0..8 {
        writeln!(source, "movdqu %xmm15, {}(%rsp)", 80 + index * 16).unwrap();
    }
    match kind {
        EntryKind::Variadic => {
            // A zero count avoids reading undefined incoming XMM registers;
            // the pre-zeroed save area remains deterministic.
            source.push_str("cmpb $8, %r11b\njbe 0f\nmovb $8, %r11b\n0:\n");
            for index in 0..8 {
                writeln!(
                    source,
                    "cmpb ${index}, %r11b\njbe 1f\nmovdqu %xmm{index}, {}(%rsp)",
                    80 + index * 16
                )
                .unwrap();
            }
            source.push_str("1:\n");
        }
        EntryKind::Fixed => {
            for index in 0..usize::from(plan.fixed_sse_used) {
                writeln!(source, "movdqu %xmm{index}, {}(%rsp)", 80 + index * 16).unwrap();
            }
        }
    }
    writeln!(
        source,
        "movl ${BRIDGE_MAGIC}, 0(%rsp)",
        BRIDGE_MAGIC = VA_STATE_MAGIC
    )
    .unwrap();
    source.push_str("movw $1, 4(%rsp)\nmovw $208, 6(%rsp)\n");
    writeln!(
        source,
        "movl ${}, 8(%rsp)",
        u32::from(plan.fixed_gp_used) * 8
    )
    .unwrap();
    writeln!(
        source,
        "movl ${}, 12(%rsp)",
        48 + u32::from(plan.fixed_sse_used) * 16
    )
    .unwrap();
    writeln!(
        source,
        "leaq {}(%rbp), %r10",
        16_u32.saturating_add(plan.overflow_arg_offset)
    )
    .unwrap();
    source.push_str("movq %r10, 16(%rsp)\nleaq 32(%rsp), %r10\nmovq %r10, 24(%rsp)\n");
    // Initialize result storage deterministically after the register saves.
    source.push_str(
        "xorq %r11, %r11\n\
         movq %r11, 208(%rsp)\n\
         movq %r11, 216(%rsp)\n\
         movq %r11, 224(%rsp)\n\
         movq %r11, 232(%rsp)\n\
         movq %r11, 240(%rsp)\n\
         movq %r11, 248(%rsp)\n\
         movq %r11, 256(%rsp)\n\
         movq %r11, 264(%rsp)\n\
         movq %rsp, %rdi\n",
    );
    writeln!(source, "call {}", plan.hidden_body_symbol).unwrap();
    if plan.hidden_return {
        source.push_str("movq 32(%rsp), %rax\n");
    } else {
        if plan.gp_results >= 1 {
            source.push_str("movq 208(%rsp), %rax\n");
        }
        if plan.gp_results >= 2 {
            source.push_str("movq 216(%rsp), %rdx\n");
        }
        if plan.xmm_results >= 1 {
            source.push_str("movdqu 224(%rsp), %xmm0\n");
        }
        if plan.xmm_results >= 2 {
            source.push_str("movdqu 240(%rsp), %xmm1\n");
        }
        if plan.x87_result {
            source.push_str("fldt 256(%rsp)\n");
        }
    }
    source.push_str("leave\n.cfi_def_cfa %rsp, 8\nret\n");
    function_footer(&plan.public_symbol, &mut source);
    GeneratedAssembly::new(
        format!(
            "{}-entry-{}",
            match kind {
                EntryKind::Fixed => "fixed",
                EntryKind::Variadic => "variadic",
            },
            short_stem(&plan.public_symbol)
        ),
        source,
        vec![plan.public_symbol.clone()],
        vec![LogicalDebugLocation::generated(plan.logical_line.max(1))],
    )
}

fn render_arm64_variadic_entry(
    plan: &BridgeEntryPlan,
    abi: AbiIdentity,
) -> Result<GeneratedAssembly, LinkError> {
    let mut source = String::new();
    target_function_header(
        &plan.public_symbol,
        plan.public_symbol_is_exact,
        plan.linkage,
        plan.weak,
        abi,
        &mut source,
    );
    source.push_str(
        "stp x29, x30, [sp, #-16]!\n\
         .cfi_def_cfa_offset 16\n\
         .cfi_offset x29, -16\n\
         .cfi_offset x30, -8\n\
         mov x29, sp\n\
         .cfi_def_cfa_register x29\n\
         sub sp, sp, #320\n\
         stp x0, x1, [sp, #48]\n\
         stp x2, x3, [sp, #64]\n\
         stp x4, x5, [sp, #80]\n\
         stp x6, x7, [sp, #96]\n\
         str q0, [sp, #112]\n\
         str q1, [sp, #128]\n\
         str q2, [sp, #144]\n\
         str q3, [sp, #160]\n\
         str q4, [sp, #176]\n\
         str q5, [sp, #192]\n\
         str q6, [sp, #208]\n\
         str q7, [sp, #224]\n\
         str x8, [sp, #40]\n",
    );
    source.push_str(
        "mov w9, #0x4343\n\
         movk w9, #0x4156, lsl #16\n\
         str w9, [sp]\n\
         mov w9, #2\n\
         strh w9, [sp, #4]\n",
    );
    if abi == AbiIdentity::Aapcs64Lp64 {
        writeln!(
            source,
            "add x9, x29, #{}",
            16_u32.saturating_add(plan.overflow_arg_offset)
        )
        .unwrap();
        source.push_str("str x9, [sp, #8]\nadd x9, sp, #112\nstr x9, [sp, #16]\nadd x9, sp, #240\nstr x9, [sp, #24]\n");
        writeln!(
            source,
            "mov w9, #{}\nstr w9, [sp, #32]",
            i32::from(plan.fixed_gp_used) * 8 - 64
        )
        .unwrap();
        writeln!(
            source,
            "mov w9, #{}\nstr w9, [sp, #36]",
            i32::from(plan.fixed_sse_used) * 16 - 128
        )
        .unwrap();
    } else {
        writeln!(
            source,
            "add x9, x29, #{}",
            16_u32.saturating_add(plan.overflow_arg_offset)
        )
        .unwrap();
        source.push_str("str x9, [sp, #8]\n");
    }
    source.push_str(
        "stp xzr, xzr, [sp, #240]\n\
         movi v31.2d, #0\n\
         str q31, [sp, #256]\n\
         str q31, [sp, #272]\n\
         str q31, [sp, #288]\n\
         str q31, [sp, #304]\n\
         mov x0, sp\n",
    );
    writeln!(
        source,
        "bl {}",
        target_symbol(&plan.hidden_body_symbol, abi)
    )
    .unwrap();
    if !plan.hidden_return {
        if plan.gp_results >= 1 {
            source.push_str("ldr x0, [sp, #240]\n");
        }
        if plan.gp_results >= 2 {
            source.push_str("ldr x1, [sp, #248]\n");
        }
        for index in 0..plan.xmm_results {
            writeln!(
                source,
                "ldr q{index}, [sp, #{}]",
                256 + u32::from(index) * 16
            )
            .unwrap();
        }
    }
    source.push_str(
        "mov sp, x29\n\
         .cfi_def_cfa sp, 16\n\
         ldp x29, x30, [sp], #16\n\
         .cfi_restore x29\n\
         .cfi_restore x30\n\
         .cfi_def_cfa sp, 0\n\
         ret\n",
    );
    target_function_footer(
        &plan.public_symbol,
        plan.public_symbol_is_exact,
        abi,
        &mut source,
    );
    GeneratedAssembly::new(
        format!("variadic-entry-{}", short_stem(&plan.public_symbol)),
        source,
        vec![plan.public_symbol.clone()],
        vec![LogicalDebugLocation::generated(plan.logical_line.max(1))],
    )
}

fn render_riscv64_variadic_entry(plan: &BridgeEntryPlan) -> Result<GeneratedAssembly, LinkError> {
    let abi = AbiIdentity::RiscvLp64d;
    let mut source = String::new();
    target_function_header(
        &plan.public_symbol,
        plan.public_symbol_is_exact,
        plan.linkage,
        plan.weak,
        abi,
        &mut source,
    );
    source.push_str(
        "addi sp, sp, -512\n\
         .cfi_def_cfa_offset 512\n\
         sd s0, 416(sp)\n\
         .cfi_offset s0, -96\n\
         sd ra, 424(sp)\n\
         .cfi_offset ra, -88\n\
         addi s0, sp, 512\n\
         sd a0, 448(sp)\n\
         sd a1, 456(sp)\n\
         sd a2, 464(sp)\n\
         sd a3, 472(sp)\n\
         sd a4, 480(sp)\n\
         sd a5, 488(sp)\n\
         sd a6, 496(sp)\n\
         sd a7, 504(sp)\n\
         fsd fa0, 112(sp)\n\
         fsd fa1, 128(sp)\n\
         fsd fa2, 144(sp)\n\
         fsd fa3, 160(sp)\n\
         fsd fa4, 176(sp)\n\
         fsd fa5, 192(sp)\n\
         fsd fa6, 208(sp)\n\
         fsd fa7, 224(sp)\n",
    );
    writeln!(source, "li t0, {VA_STATE_MAGIC}\nsw t0, 0(sp)").unwrap();
    source.push_str("li t0, 2\nsh t0, 4(sp)\n");
    if plan.fixed_gp_used < 8 {
        writeln!(
            source,
            "addi t0, sp, {}",
            448 + u32::from(plan.fixed_gp_used) * 8
        )
        .unwrap();
    } else if plan.overflow_arg_offset == 0 {
        source.push_str("mv t0, s0\n");
    } else {
        writeln!(
            source,
            "li t0, {}\nadd t0, s0, t0",
            plan.overflow_arg_offset
        )
        .unwrap();
    }
    source.push_str(
        "sd t0, 8(sp)\n\
         sd zero, 240(sp)\n\
         sd zero, 248(sp)\n\
         sd zero, 256(sp)\n\
         sd zero, 264(sp)\n\
         sd zero, 272(sp)\n\
         sd zero, 280(sp)\n\
         sd zero, 288(sp)\n\
         sd zero, 296(sp)\n\
         sd zero, 304(sp)\n\
         sd zero, 312(sp)\n\
         li t0, -1\n\
         sw t0, 260(sp)\n\
         sw t0, 276(sp)\n\
         sw t0, 292(sp)\n\
         sw t0, 308(sp)\n\
         mv a0, sp\n",
    );
    writeln!(source, "call {}", plan.hidden_body_symbol).unwrap();
    if !plan.hidden_return {
        if plan.gp_results >= 1 {
            source.push_str("ld a0, 240(sp)\n");
        }
        if plan.gp_results >= 2 {
            source.push_str("ld a1, 248(sp)\n");
        }
        for index in 0..plan.xmm_results {
            writeln!(source, "fld fa{index}, {}(sp)", 256 + u32::from(index) * 16).unwrap();
        }
    }
    source.push_str(
        "ld s0, 416(sp)\n\
         ld ra, 424(sp)\n\
         addi sp, sp, 512\n\
         .cfi_def_cfa_offset 0\n\
         ret\n",
    );
    target_function_footer(
        &plan.public_symbol,
        plan.public_symbol_is_exact,
        abi,
        &mut source,
    );
    GeneratedAssembly::new(
        format!("variadic-entry-{}", short_stem(&plan.public_symbol)),
        source,
        vec![plan.public_symbol.clone()],
        vec![LogicalDebugLocation::generated(plan.logical_line.max(1))],
    )
}

fn short_stem(symbol: &str) -> String {
    let mut stem = symbol
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        .take(48)
        .map(char::from)
        .collect::<String>();
    if stem.is_empty() {
        stem.push_str("entry");
    }
    stem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_assembly_support_is_exact_deterministic_and_abi_safe() {
        let plan = InlineAsmSupportPlan {
            cpuid_symbol: Some("__ccc_support_cpuid_test".to_owned()),
            rdtsc_symbol: Some("__ccc_support_rdtsc_test".to_owned()),
        };
        let assembly = render_inline_asm_support(&plan).unwrap();
        let source = assembly.source();

        assert_eq!(assembly.stem(), "inline-asm-support");
        assert_eq!(
            assembly.defined_symbols(),
            ["__ccc_support_cpuid_test", "__ccc_support_rdtsc_test"]
        );
        assert_eq!(assembly, render_inline_asm_support(&plan).unwrap());
        assert!(source.contains(".hidden __ccc_support_cpuid_test"));
        assert!(source.contains(".hidden __ccc_support_rdtsc_test"));
        assert!(source.contains("pushq %rbx\n.cfi_def_cfa_offset 16"));
        assert!(source.contains(".cfi_offset %rbx, -16"));
        assert!(source.contains("movq %rdx, %r10\nmovq %rcx, %r11"));
        assert!(source.contains("movl %edi, %eax\nmovl %esi, %ecx\ncpuid"));
        assert!(source.contains("rdtsc\nmovl %eax, (%rdi)\nmovl %edx, (%rsi)"));
        assert!(source.contains(".section .note.GNU-stack,\"\",@progbits"));
    }

    #[test]
    fn inline_assembly_support_rejects_empty_or_foreign_plans() {
        let error = render_inline_asm_support(&InlineAsmSupportPlan {
            cpuid_symbol: None,
            rdtsc_symbol: None,
        })
        .unwrap_err();
        assert!(error.message.contains("at least one helper"));

        for plan in [
            InlineAsmSupportPlan {
                cpuid_symbol: Some("cpuid".to_owned()),
                rdtsc_symbol: None,
            },
            InlineAsmSupportPlan {
                cpuid_symbol: None,
                rdtsc_symbol: Some("rdtsc".to_owned()),
            },
        ] {
            let error = render_inline_asm_support(&plan).unwrap_err();
            assert!(error.message.contains("reserved compiler namespace"));
        }
    }

    #[test]
    fn protocol_layouts_are_exact() {
        assert_eq!(size_of::<BridgeFrameV2>(), 272);
        assert_eq!(offset_of!(BridgeFrameV2, gp_slots), 32);
        assert_eq!(offset_of!(BridgeFrameV2, xmm_slots), 80);
        assert_eq!(offset_of!(BridgeFrameV2, gp_result_slots), 208);
        assert_eq!(offset_of!(BridgeFrameV2, xmm_result_slots), 224);
        assert_eq!(offset_of!(BridgeFrameV2, x87_result_slot), 256);
        assert_eq!(size_of::<VaStateV1>(), 208);
        assert_eq!(offset_of!(VaStateV1, gp_offset), 8);
        assert_eq!(offset_of!(VaStateV1, register_save_area), 32);
    }

    #[test]
    fn variadic_sse_count_saturates_at_eight() {
        for (actual, expected) in [(0, 0), (1, 1), (8, 8), (9, 8), (u8::MAX, 8)] {
            let mut frame = BridgeFrameV2::zeroed(0, 0);
            frame.set_variadic_sse_count(actual);
            assert_eq!(frame.variadic_sse_count, expected);
        }
    }

    #[test]
    fn generated_names_include_a_full_sha256_digest() {
        let name = generated_symbol_name(
            &[0x42; 32],
            GeneratedSymbolKind::CallHelper,
            b"function:7/call:2",
            b"canonical-plan",
        );
        assert!(name.starts_with("__ccc_call_helper_"));
        assert_eq!(name.len(), "__ccc_call_helper_".len() + 64);
        assert_eq!(
            name,
            generated_symbol_name(
                &[0x42; 32],
                GeneratedSymbolKind::CallHelper,
                b"function:7/call:2",
                b"canonical-plan",
            )
        );
        assert_ne!(
            name,
            generated_symbol_name(
                &[0x42; 32],
                GeneratedSymbolKind::CallHelper,
                b"function:7/call:3",
                b"canonical-plan",
            )
        );
    }

    #[test]
    fn tls_accessors_render_canonical_elf_models_and_unwind_state() {
        let helper = "__ccc_tls_accessor_0123456789abcdef";
        for (model, required) in [
            (
                TlsAccessModel::GeneralDynamic,
                &[
                    "data16 leaq tls_value@TLSGD(%rip), %rdi",
                    ".value 0x6666\nrex64\ncall __tls_get_addr@PLT",
                ][..],
            ),
            (
                TlsAccessModel::LocalDynamic,
                &[
                    "leaq tls_value@TLSLD(%rip), %rdi",
                    "call __tls_get_addr@PLT",
                    "leaq tls_value@DTPOFF(%rax), %rax",
                ][..],
            ),
            (
                TlsAccessModel::InitialExec,
                &["movq %fs:0, %rax", "addq tls_value@GOTTPOFF(%rip), %rax"][..],
            ),
            (
                TlsAccessModel::LocalExec,
                &["movq %fs:0, %rax", "leaq tls_value@TPOFF(%rax), %rax"][..],
            ),
        ] {
            let assembly = render_target_tls_accessor(&TlsAccessorPlan {
                abi: AbiIdentity::SysvAmd64Lp64,
                helper_symbol: helper.to_owned(),
                object_symbol: "tls_value".to_owned(),
                object_symbol_is_exact: false,
                model,
                object_visibility: TlsSymbolVisibility::Default,
                logical_line: 19,
            })
            .unwrap();
            for instruction in required {
                assert!(
                    assembly.source().contains(instruction),
                    "missing `{instruction}` in:\n{}",
                    assembly.source()
                );
            }
            assert!(assembly.source().contains(".hidden __ccc_tls_accessor_"));
            assert!(assembly.source().contains(".cfi_startproc"));
            assert!(assembly.source().contains(".cfi_endproc"));
            assert!(assembly.source().contains(".note.GNU-stack"));
            assert_eq!(assembly.debug_locations()[0].line, 19);
        }
    }

    #[test]
    fn dynamic_tls_accessors_align_calls_and_general_dynamic_is_relaxable() {
        let assembly = render_target_tls_accessor(&TlsAccessorPlan {
            abi: AbiIdentity::SysvAmd64Lp64,
            helper_symbol: "__ccc_tls_accessor_dynamic".to_owned(),
            object_symbol: "value".to_owned(),
            object_symbol_is_exact: false,
            model: TlsAccessModel::GeneralDynamic,
            object_visibility: TlsSymbolVisibility::Hidden,
            logical_line: 1,
        })
        .unwrap();
        let source = assembly.source();
        let subtract = source.find("subq $8, %rsp").unwrap();
        let call = source.find("call __tls_get_addr@PLT").unwrap();
        let add = source.find("addq $8, %rsp").unwrap();
        assert!(subtract < call && call < add, "{source}");
        assert!(source.contains(".cfi_def_cfa_offset 16"));
        assert!(source.contains(".cfi_def_cfa_offset 8"));
        assert!(source.contains(".hidden value"));
        assert!(source.contains("data16 leaq"));
        assert!(source.contains(".value 0x6666\nrex64\ncall"));
    }

    #[test]
    fn linux_arm64_tls_accessors_use_target_relocation_families() {
        for (model, required) in [
            (
                TlsAccessModel::GeneralDynamic,
                &[":tlsdesc:value", ".tlsdesccall value", "mrs x8, TPIDR_EL0"][..],
            ),
            (
                TlsAccessModel::LocalDynamic,
                &[":tlsdesc:value", ".tlsdesccall value", "mrs x8, TPIDR_EL0"][..],
            ),
            (
                TlsAccessModel::InitialExec,
                &[":gottprel:value", ":gottprel_lo12:value"][..],
            ),
            (
                TlsAccessModel::LocalExec,
                &[":tprel_hi12:value", ":tprel_lo12_nc:value"][..],
            ),
        ] {
            let assembly = render_target_tls_accessor(&TlsAccessorPlan {
                abi: AbiIdentity::Aapcs64Lp64,
                helper_symbol: "__ccc_tls_accessor_arm64".to_owned(),
                object_symbol: "value".to_owned(),
                object_symbol_is_exact: false,
                model,
                object_visibility: TlsSymbolVisibility::Protected,
                logical_line: 1,
            })
            .unwrap();
            for fragment in required {
                assert!(
                    assembly.source().contains(fragment),
                    "{}",
                    assembly.source()
                );
            }
            assert!(assembly.source().contains(".protected value"));
            assert!(assembly.source().contains(".note.GNU-stack"));
        }
    }

    #[test]
    fn linux_riscv64_tls_accessors_use_target_relocation_families() {
        for (model, required) in [
            (
                TlsAccessModel::GeneralDynamic,
                &["%tls_gd_pcrel_hi(value)", "call __tls_get_addr"][..],
            ),
            (
                TlsAccessModel::LocalDynamic,
                &["%tls_gd_pcrel_hi(value)", "call __tls_get_addr"][..],
            ),
            (
                TlsAccessModel::InitialExec,
                &["%tls_ie_pcrel_hi(value)", "add a0, a0, tp"][..],
            ),
            (
                TlsAccessModel::LocalExec,
                &["%tprel_hi(value)", "%tprel_add(value)", "%tprel_lo(value)"][..],
            ),
        ] {
            let assembly = render_target_tls_accessor(&TlsAccessorPlan {
                abi: AbiIdentity::RiscvLp64d,
                helper_symbol: "__ccc_tls_accessor_riscv64".to_owned(),
                object_symbol: "value".to_owned(),
                object_symbol_is_exact: false,
                model,
                object_visibility: TlsSymbolVisibility::Internal,
                logical_line: 1,
            })
            .unwrap();
            for fragment in required {
                assert!(
                    assembly.source().contains(fragment),
                    "{}",
                    assembly.source()
                );
            }
            assert!(assembly.source().contains(".internal value"));
            assert!(assembly.source().contains(".note.GNU-stack"));
        }
    }

    #[test]
    fn darwin_arm64_tls_accessor_uses_tlv_and_respects_exact_symbols() {
        for (exact, reference) in [(false, "_value@TLVPPAGE"), (true, "value@TLVPPAGE")] {
            let assembly = render_target_tls_accessor(&TlsAccessorPlan {
                abi: AbiIdentity::DarwinArm64,
                helper_symbol: "__ccc_tls_accessor_darwin".to_owned(),
                object_symbol: "value".to_owned(),
                object_symbol_is_exact: exact,
                model: TlsAccessModel::InitialExec,
                object_visibility: TlsSymbolVisibility::Hidden,
                logical_line: 1,
            })
            .unwrap();
            assert!(
                assembly.source().contains(reference),
                "{}",
                assembly.source()
            );
            assert!(assembly.source().contains("ldr x8, [x0]\nblr x8"));
            assert!(assembly.source().contains("___ccc_tls_accessor_darwin:"));
            assert!(
                assembly
                    .source()
                    .contains(".private_extern ___ccc_tls_accessor_darwin")
            );
            assert!(assembly.source().contains(".subsections_via_symbols"));
            assert!(!assembly.source().contains(".note.GNU-stack"));
        }
    }

    #[test]
    fn x86_call_helper_guards_live_prefixes_and_writes_al_last() {
        let assembly = render_generic_call_helper("__ccc_call_helper_test").unwrap();
        let source = assembly.source();
        for index in 0..8 {
            assert!(
                source.contains(&format!("movdqu {}(%r12), %xmm{index}", 80 + index * 16)),
                "{source}"
            );
            assert!(
                source.contains(&format!("cmpb ${index}, %r10b")),
                "{source}"
            );
        }
        for (index, register) in ["%rdi", "%rsi", "%rdx", "%rcx", "%r8", "%r9"]
            .into_iter()
            .enumerate()
        {
            assert!(
                source.contains(&format!("movq {}(%r12), {register}", 32 + index * 8)),
                "{source}"
            );
        }
        let al_instruction = "movzbl 26(%r12), %eax";
        let al = source.find(al_instruction).unwrap();
        let call = source.find("call *%r11").unwrap();
        assert!(al < call);
        assert_eq!(
            &source[al..call + "call *%r11".len()],
            "movzbl 26(%r12), %eax\ncall *%r11"
        );
        assert!(source.contains("movzbl 25(%r12), %r10d"));
        assert!(source.contains("jbe .Lccc_call_xmm_inputs_done"));
        assert!(source.contains("movzbl 24(%r12), %r10d"));
        assert!(source.contains("jbe .Lccc_call_gp_inputs_done"));
        assert!(source.contains(".cfi_startproc"));
        assert!(source.contains(".note.GNU-stack"));
        assert!(!source.contains(".file"));
        assert!(!source.contains(".loc"));
        assert!(!source.contains("ud2"));
        assert!(!source.contains("ldmxcsr"));
        assert!(!source.contains("fldcw"));
        assert!(!source.contains("cld"));
        assert!(source.contains("leaq 272(%r12), %rsi"));
        assert!(source.contains("testb $1, 29(%r12)"));
        assert!(source.contains("fstpt 256(%r12)"));
    }

    #[test]
    fn arm64_call_helper_guards_live_prefixes_and_always_restores_x8() {
        for abi in [AbiIdentity::Aapcs64Lp64, AbiIdentity::DarwinArm64] {
            let assembly = render_target_call_helper("__ccc_call_helper_arm64_test", abi).unwrap();
            let source = assembly.source();
            assert!(source.contains("ldrb w17, [x19, #25]"), "{source}");
            assert!(source.contains("adr x9, 3f"));
            assert!(source.contains("ldrb w17, [x19, #24]"), "{source}");
            assert!(source.contains("adr x9, 4f"));
            assert_eq!(source.matches("sub x9, x9, x17, lsl #2").count(), 2);
            assert_eq!(source.matches("br x9").count(), 2);
            for index in 0..8 {
                assert!(
                    source.contains(&format!("ldr q{index}, [x19, #{}]", 112 + index * 16)),
                    "{source}"
                );
                assert!(
                    source.contains(&format!("ldr x{index}, [x19, #{}]", 48 + index * 8)),
                    "{source}"
                );
            }
            let x8 = source.find("ldr x8, [x19, #32]").unwrap();
            let call = source.find("blr x16").unwrap();
            assert!(x8 < call, "{source}");
        }
    }

    #[test]
    fn f80_support_keeps_ordered_operations_and_control_word_changes_local() {
        let assembly =
            render_f80_support("__ccc_support_f80_test", F80RuntimeHelperPlan::default()).unwrap();
        let source = assembly.source();
        assert_eq!(assembly.defined_symbols(), ["__ccc_support_f80_test"]);
        assert!(source.contains("fsubrp %st, %st(1)"), "{source}");
        assert!(source.contains("fdivrp %st, %st(1)"), "{source}");
        assert!(!source.contains("fsubp %st, %st(1)"), "{source}");
        assert!(!source.contains("fdivp %st, %st(1)"), "{source}");
        assert!(source.contains("fucomip %st(1), %st"), "{source}");
        assert!(source.contains("fcomip %st(1), %st"), "{source}");
        assert!(source.contains("cmpl $21, %eax"), "{source}");
        assert!(source.contains("fnstcw 8(%rsp)"), "{source}");
        assert!(source.contains("fldcw 8(%rsp)"), "{source}");
        assert!(source.contains(".Lccc_f80_two63:"), "{source}");
        assert!(source.contains(".note.GNU-stack"), "{source}");
    }

    #[test]
    fn f80_wide_conversions_select_only_the_requested_runtime_helpers() {
        let assembly = render_f80_support(
            "__ccc_support_f80_test",
            F80RuntimeHelperPlan {
                from_i128: true,
                from_u128: false,
                to_i128: false,
                to_u128: true,
            },
        )
        .unwrap();
        let source = assembly.source();
        assert!(source.contains("call __floattixf@PLT"), "{source}");
        assert!(source.contains("call __fixunsxfti@PLT"), "{source}");
        assert!(!source.contains("__floatuntixf"), "{source}");
        assert!(!source.contains("__fixxfti"), "{source}");
        assert!(source.contains(".cfi_offset %r12, -16"), "{source}");
        assert!(source.contains("movq %rdx, 8(%r12)"), "{source}");
    }

    #[test]
    fn fixed_entry_restores_an_x87_result_immediately_before_return() {
        let assembly = render_target_fixed_entry(
            &BridgeEntryPlan {
                public_symbol: "x87_identity".to_owned(),
                public_symbol_is_exact: false,
                hidden_body_symbol: "__ccc_fixed_body_x87".to_owned(),
                linkage: AssemblyFunctionLinkage::ExternalDefault,
                weak: false,
                fixed_gp_used: 0,
                fixed_sse_used: 0,
                overflow_arg_offset: 16,
                gp_results: 0,
                xmm_results: 0,
                x87_result: true,
                hidden_return: false,
                logical_line: 1,
            },
            AbiIdentity::SysvAmd64Lp64,
        )
        .unwrap();
        let source = assembly.source();
        let call = source.find("call __ccc_fixed_body_x87").unwrap();
        let load = source.find("fldt 256(%rsp)").unwrap();
        let leave = source.find("leave").unwrap();
        assert!(call < load && load < leave, "{source}");
    }

    #[test]
    fn riscv_call_helper_uses_a_stable_cfa_across_dynamic_outgoing_storage() {
        let assembly =
            render_target_call_helper("__ccc_call_helper_riscv_test", AbiIdentity::RiscvLp64d)
                .unwrap();
        let source = assembly.source();
        let establish = source.find("addi s0, sp, 32").unwrap();
        let stable_cfa = source[establish..].find(".cfi_def_cfa s0, 0").unwrap() + establish;
        let dynamic = source.find("sub sp, sp, s2").unwrap();
        let restore_sp = source.find("addi sp, s0, -32").unwrap();
        let restore_cfa = source[restore_sp..].find(".cfi_def_cfa sp, 32").unwrap() + restore_sp;
        let call = source.find("jalr t0").unwrap();
        assert!(establish < stable_cfa && stable_cfa < dynamic && dynamic < call);
        assert!(call < restore_sp && restore_sp < restore_cfa);
        assert!(source.contains(".cfi_def_cfa sp, 0"));
        assert!(
            source.contains("lbu t1, 25(s1)\nbeqz t1, .Lccc_call_riscv_fp_inputs_done"),
            "{source}"
        );
        assert!(
            source.contains("lbu t1, 24(s1)\nbeqz t1, .Lccc_call_riscv_gp_inputs_done"),
            "{source}"
        );
        for index in 0..8 {
            assert!(
                source.contains(&format!("fld fa{index}, {}(s1)", 112 + index * 16)),
                "{source}"
            );
            assert!(
                source.contains(&format!("ld a{index}, {}(s1)", 48 + index * 8)),
                "{source}"
            );
        }
    }

    #[test]
    fn variadic_entry_builds_the_public_va_list_view() {
        let assembly = render_variadic_entry(&BridgeEntryPlan {
            public_symbol: "consume".to_owned(),
            public_symbol_is_exact: false,
            hidden_body_symbol: "__ccc_variadic_body_test".to_owned(),
            linkage: AssemblyFunctionLinkage::ExternalHidden,
            weak: false,
            fixed_gp_used: 2,
            fixed_sse_used: 1,
            overflow_arg_offset: 16,
            gp_results: 1,
            xmm_results: 0,
            x87_result: false,
            hidden_return: false,
            logical_line: 27,
        })
        .unwrap();
        let source = assembly.source();
        assert!(source.contains("subq $272, %rsp"));
        assert!(source.contains("movl $16, 8(%rsp)"));
        assert!(source.contains("movl $64, 12(%rsp)"));
        assert!(source.contains("leaq 32(%rsp), %r10"));
        assert!(source.contains("movq %rsp, %rdi"));
        assert!(source.contains("call __ccc_variadic_body_test"));
        assert!(source.contains("movzbl %al, %r11d"));
        assert!(source.contains("cmpb $8, %r11b\njbe 0f\nmovb $8, %r11b"));
        assert!(source.contains("movq 208(%rsp), %rax"));
        assert!(!source.contains(".file"));
        assert!(!source.contains(".loc"));
        assert!(source.contains(".hidden consume"));
    }

    #[test]
    fn fixed_entry_saves_planned_sse_inputs_without_reading_variadic_metadata() {
        let assembly = render_target_fixed_entry(
            &BridgeEntryPlan {
                public_symbol: "fixed_wide".to_owned(),
                public_symbol_is_exact: false,
                hidden_body_symbol: "__ccc_fixed_body_test".to_owned(),
                linkage: AssemblyFunctionLinkage::ExternalDefault,
                weak: true,
                fixed_gp_used: 2,
                fixed_sse_used: 2,
                overflow_arg_offset: 0,
                gp_results: 2,
                xmm_results: 0,
                x87_result: false,
                hidden_return: false,
                logical_line: 1,
            },
            AbiIdentity::SysvAmd64Lp64,
        )
        .unwrap();
        let source = assembly.source();
        assert!(source.contains(".weak fixed_wide"), "{source}");
        assert!(!source.contains(".globl fixed_wide"), "{source}");
        assert!(!source.contains("%al"), "{source}");
        assert!(!source.contains("%r11b"), "{source}");
        assert!(source.contains("movdqu %xmm0, 80(%rsp)"), "{source}");
        assert!(source.contains("movdqu %xmm1, 96(%rsp)"), "{source}");
        assert!(!source.contains("movdqu %xmm2, 112(%rsp)"), "{source}");
    }

    #[test]
    fn variadic_entry_rejects_conflicting_indirect_and_register_results() {
        let error = render_variadic_entry(&BridgeEntryPlan {
            public_symbol: "consume".to_owned(),
            public_symbol_is_exact: false,
            hidden_body_symbol: "hidden".to_owned(),
            linkage: AssemblyFunctionLinkage::Internal,
            weak: false,
            fixed_gp_used: 2,
            fixed_sse_used: 0,
            overflow_arg_offset: 0,
            gp_results: 1,
            xmm_results: 0,
            x87_result: false,
            hidden_return: true,
            logical_line: 1,
        })
        .unwrap_err();
        assert!(
            error
                .message
                .contains("cannot also request register results")
        );
    }

    #[test]
    fn bridge_entries_reject_x87_results_outside_the_x86_scalar_contract() {
        let plan = BridgeEntryPlan {
            public_symbol: "consume".to_owned(),
            public_symbol_is_exact: false,
            hidden_body_symbol: "hidden".to_owned(),
            linkage: AssemblyFunctionLinkage::Internal,
            weak: false,
            fixed_gp_used: 0,
            fixed_sse_used: 0,
            overflow_arg_offset: 0,
            gp_results: 0,
            xmm_results: 0,
            x87_result: true,
            hidden_return: false,
            logical_line: 1,
        };
        let error = render_target_variadic_entry(&plan, AbiIdentity::Aapcs64Lp64).unwrap_err();
        assert!(error.message.contains("sole System V AMD64 result"));

        let mut mixed = plan;
        mixed.gp_results = 1;
        let error = render_variadic_entry(&mixed).unwrap_err();
        assert!(error.message.contains("sole System V AMD64 result"));
    }

    #[test]
    fn variadic_entry_retains_internal_source_linkage() {
        let assembly = render_variadic_entry(&BridgeEntryPlan {
            public_symbol: "local_consume".to_owned(),
            public_symbol_is_exact: false,
            hidden_body_symbol: "__ccc_variadic_body_local".to_owned(),
            linkage: AssemblyFunctionLinkage::Internal,
            weak: false,
            fixed_gp_used: 0,
            fixed_sse_used: 0,
            overflow_arg_offset: 0,
            gp_results: 0,
            xmm_results: 0,
            x87_result: false,
            hidden_return: false,
            logical_line: 1,
        })
        .unwrap();
        assert!(assembly.source().contains(".globl local_consume"));
        assert!(assembly.source().contains(".hidden local_consume"));
    }

    #[test]
    fn variadic_entry_emits_each_external_elf_visibility_directive() {
        for (linkage, directive) in [
            (AssemblyFunctionLinkage::ExternalDefault, None),
            (
                AssemblyFunctionLinkage::ExternalHidden,
                Some(".hidden consume"),
            ),
            (
                AssemblyFunctionLinkage::ExternalProtected,
                Some(".protected consume"),
            ),
            (
                AssemblyFunctionLinkage::ExternalInternal,
                Some(".internal consume"),
            ),
        ] {
            let assembly = render_variadic_entry(&BridgeEntryPlan {
                public_symbol: "consume".to_owned(),
                public_symbol_is_exact: false,
                hidden_body_symbol: "__ccc_variadic_body_visibility".to_owned(),
                linkage,
                weak: false,
                fixed_gp_used: 0,
                fixed_sse_used: 0,
                overflow_arg_offset: 0,
                gp_results: 1,
                xmm_results: 0,
                x87_result: false,
                hidden_return: false,
                logical_line: 1,
            })
            .unwrap();
            let source = assembly.source();
            assert!(source.contains(".globl consume"));
            for possible in [".hidden consume", ".protected consume", ".internal consume"] {
                assert_eq!(
                    source.contains(possible),
                    directive == Some(possible),
                    "{linkage:?}:\n{source}"
                );
            }
        }
    }

    #[test]
    fn darwin_variadic_entry_preserves_exact_assembly_label_spelling() {
        let render = |public_symbol: &str, public_symbol_is_exact: bool| {
            render_target_variadic_entry(
                &BridgeEntryPlan {
                    public_symbol: public_symbol.to_owned(),
                    public_symbol_is_exact,
                    hidden_body_symbol: "__ccc_variadic_body_darwin_label".to_owned(),
                    linkage: AssemblyFunctionLinkage::ExternalDefault,
                    weak: false,
                    fixed_gp_used: 1,
                    fixed_sse_used: 0,
                    overflow_arg_offset: 0,
                    gp_results: 1,
                    xmm_results: 0,
                    x87_result: false,
                    hidden_return: false,
                    logical_line: 1,
                },
                AbiIdentity::DarwinArm64,
            )
            .unwrap()
        };

        let exact = render("_physical_variadic", true);
        assert!(exact.source().contains(".globl _physical_variadic\n"));
        assert!(exact.source().contains("_physical_variadic:\n"));
        assert!(!exact.source().contains("__physical_variadic"));

        let exact_without_prefix = render("physical_variadic", true);
        assert!(
            exact_without_prefix
                .source()
                .contains(".globl physical_variadic\n")
        );
        assert!(
            exact_without_prefix
                .source()
                .contains("physical_variadic:\n")
        );
        assert!(!exact_without_prefix.source().contains("_physical_variadic"));

        let ordinary = render("_ordinary_variadic", false);
        assert!(ordinary.source().contains(".globl __ordinary_variadic\n"));
        assert!(ordinary.source().contains("__ordinary_variadic:\n"));
    }

    #[test]
    fn variadic_entry_guards_each_xmm_save_with_the_incoming_al_bound() {
        let assembly = render_variadic_entry(&BridgeEntryPlan {
            public_symbol: "consume".to_owned(),
            public_symbol_is_exact: false,
            hidden_body_symbol: "__ccc_variadic_body_al_bound".to_owned(),
            linkage: AssemblyFunctionLinkage::ExternalDefault,
            weak: false,
            fixed_gp_used: 0,
            fixed_sse_used: 0,
            overflow_arg_offset: 0,
            gp_results: 1,
            xmm_results: 0,
            x87_result: false,
            hidden_return: false,
            logical_line: 1,
        })
        .unwrap();
        let source = assembly.source();
        for index in 0..8 {
            let guarded_save = format!(
                "cmpb ${index}, %r11b\njbe 1f\nmovdqu %xmm{index}, {}(%rsp)",
                80 + index * 16
            );
            assert!(source.contains(&guarded_save), "{guarded_save}\n{source}");
        }

        let simulated_saves = |incoming_al: u8| {
            let bounded = incoming_al.min(8);
            (0_u8..8)
                .filter(|index| bounded > *index)
                .collect::<Vec<_>>()
        };
        assert_eq!(simulated_saves(0), Vec::<u8>::new());
        assert_eq!(simulated_saves(1), vec![0]);
        assert_eq!(simulated_saves(8), (0_u8..8).collect::<Vec<_>>());
        assert_eq!(simulated_saves(u8::MAX), (0_u8..8).collect::<Vec<_>>());
    }
}
