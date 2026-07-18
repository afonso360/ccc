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

/// The fixed portion of the call-helper protocol.
///
/// The trailing outgoing stack payload starts immediately after this value.
/// Producers zero the complete allocation before populating live slots, which
/// lets the assembly helper load every register slot without data-dependent
/// dispatch.
#[repr(C, align(16))]
#[derive(Clone)]
pub struct BridgeFrameV1 {
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
}

impl BridgeFrameV1 {
    pub const VERSION: u16 = 1;
    pub const HEADER_SIZE: u16 = 32;
    pub const FIXED_SIZE: usize = 256;
    pub const ALIGNMENT: usize = 16;

    /// Creates a zeroed frame header. The caller owns any trailing stack area.
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
pub struct VariadicEntryFrameV1 {
    pub va_state: VaStateV1,
    pub gp_result_slots: [[u8; 8]; 2],
    pub xmm_result_slots: [[u8; 16]; 2],
}

impl VariadicEntryFrameV1 {
    pub const SIZE: usize = 256;
    pub const GP_RESULTS_OFFSET: usize = 208;
    pub const XMM_RESULTS_OFFSET: usize = 224;

    pub fn zeroed() -> Self {
        Self {
            va_state: VaStateV1::zeroed(),
            gp_result_slots: [[0; 8]; 2],
            xmm_result_slots: [[0; 16]; 2],
        }
    }
}

const _: () = {
    assert!(size_of::<BridgeFrameV1>() == BridgeFrameV1::FIXED_SIZE);
    assert!(align_of::<BridgeFrameV1>() == BridgeFrameV1::ALIGNMENT);
    assert!(offset_of!(BridgeFrameV1, target_address) == 8);
    assert!(offset_of!(BridgeFrameV1, gp_slots) == 32);
    assert!(offset_of!(BridgeFrameV1, xmm_slots) == 80);
    assert!(offset_of!(BridgeFrameV1, gp_result_slots) == 208);
    assert!(offset_of!(BridgeFrameV1, xmm_result_slots) == 224);
    assert!(size_of::<VaStateV1>() == VaStateV1::SIZE);
    assert!(offset_of!(VaStateV1, gp_offset) == VaStateV1::VA_LIST_OFFSET);
    assert!(offset_of!(VaStateV1, register_save_area) == 32);
    assert!(size_of::<VariadicEntryFrameV1>() == VariadicEntryFrameV1::SIZE);
    assert!(offset_of!(VariadicEntryFrameV1, gp_result_slots) == 208);
    assert!(offset_of!(VariadicEntryFrameV1, xmm_result_slots) == 224);
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GeneratedSymbolKind {
    CallHelper,
    CallStub,
    VariadicEntry,
    VariadicBody,
    Support,
}

impl GeneratedSymbolKind {
    fn tag(self) -> &'static str {
        match self {
            Self::CallHelper => "call-helper",
            Self::CallStub => "call-stub",
            Self::VariadicEntry => "variadic-entry",
            Self::VariadicBody => "variadic-body",
            Self::Support => "support",
        }
    }

    fn identifier_component(self) -> &'static str {
        match self {
            Self::CallHelper => "call_helper",
            Self::CallStub => "call_stub",
            Self::VariadicEntry => "variadic_entry",
            Self::VariadicBody => "variadic_body",
            Self::Support => "support",
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
        "__ccc_variadic_entry_",
        "__ccc_variadic_body_",
        "__ccc_support_",
    ]
    .iter()
    .any(|prefix| symbol.starts_with(prefix))
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

fn function_header(symbol: &str, linkage: AssemblyFunctionLinkage, output: &mut String) {
    output.push_str(".p2align 4\n");
    match linkage {
        AssemblyFunctionLinkage::ExternalDefault => {
            writeln!(output, ".globl {symbol}").unwrap();
        }
        AssemblyFunctionLinkage::ExternalHidden => {
            writeln!(output, ".globl {symbol}").unwrap();
            writeln!(output, ".hidden {symbol}").unwrap();
        }
        AssemblyFunctionLinkage::ExternalProtected => {
            writeln!(output, ".globl {symbol}").unwrap();
            writeln!(output, ".protected {symbol}").unwrap();
        }
        AssemblyFunctionLinkage::ExternalInternal => {
            writeln!(output, ".globl {symbol}").unwrap();
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
    if abi == AbiIdentity::DarwinArm64 {
        format!("_{symbol}")
    } else {
        symbol.to_owned()
    }
}

fn target_function_header(
    symbol: &str,
    linkage: AssemblyFunctionLinkage,
    abi: AbiIdentity,
    output: &mut String,
) {
    output.push_str(".text\n.p2align 2\n");
    let assembly_symbol = target_symbol(symbol, abi);
    if abi == AbiIdentity::DarwinArm64 {
        if linkage != AssemblyFunctionLinkage::Internal {
            writeln!(output, ".globl {assembly_symbol}").unwrap();
        } else {
            // The primary Mach-O object references this definition, so it is
            // private external until the relocatable link resolves the pair.
            writeln!(output, ".globl {assembly_symbol}").unwrap();
        }
        if linkage != AssemblyFunctionLinkage::ExternalDefault {
            writeln!(output, ".private_extern {assembly_symbol}").unwrap();
        }
    } else {
        match linkage {
            AssemblyFunctionLinkage::ExternalDefault => {
                writeln!(output, ".globl {assembly_symbol}").unwrap();
            }
            AssemblyFunctionLinkage::ExternalHidden | AssemblyFunctionLinkage::Internal => {
                writeln!(output, ".globl {assembly_symbol}").unwrap();
                writeln!(output, ".hidden {assembly_symbol}").unwrap();
            }
            AssemblyFunctionLinkage::ExternalProtected => {
                writeln!(output, ".globl {assembly_symbol}").unwrap();
                writeln!(output, ".protected {assembly_symbol}").unwrap();
            }
            AssemblyFunctionLinkage::ExternalInternal => {
                writeln!(output, ".globl {assembly_symbol}").unwrap();
                writeln!(output, ".internal {assembly_symbol}").unwrap();
            }
        }
        writeln!(output, ".type {assembly_symbol}, @function").unwrap();
    }
    writeln!(output, "{assembly_symbol}:").unwrap();
    output.push_str(".cfi_startproc\n");
}

fn target_function_footer(symbol: &str, abi: AbiIdentity, output: &mut String) {
    output.push_str(".cfi_endproc\n");
    let assembly_symbol = target_symbol(symbol, abi);
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
    function_header(symbol, AssemblyFunctionLinkage::ExternalHidden, &mut source);
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
         leaq 256(%r12), %rsi\n\
         movl 16(%r12), %ecx\n\
         rep movsb\n\
         movq 8(%r12), %r11\n\
         movdqu 80(%r12), %xmm0\n\
         movdqu 96(%r12), %xmm1\n\
         movdqu 112(%r12), %xmm2\n\
         movdqu 128(%r12), %xmm3\n\
         movdqu 144(%r12), %xmm4\n\
         movdqu 160(%r12), %xmm5\n\
         movdqu 176(%r12), %xmm6\n\
         movdqu 192(%r12), %xmm7\n\
         movq 32(%r12), %rdi\n\
         movq 40(%r12), %rsi\n\
         movq 48(%r12), %rdx\n\
         movq 56(%r12), %rcx\n\
         movq 64(%r12), %r8\n\
         movq 72(%r12), %r9\n\
         movzbl 26(%r12), %eax\n\
         call *%r11\n\
         movq %rax, 208(%r12)\n\
         movq %rdx, 216(%r12)\n\
         movdqu %xmm0, 224(%r12)\n\
         movdqu %xmm1, 240(%r12)\n\
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

fn render_arm64_call_helper(
    symbol: &str,
    abi: AbiIdentity,
) -> Result<GeneratedAssembly, LinkError> {
    validate_symbol(symbol)?;
    let mut source = String::new();
    target_function_header(
        symbol,
        AssemblyFunctionLinkage::ExternalHidden,
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
         ldr x16, [x19, #8]\n\
         ldr q0, [x19, #112]\n\
         ldr q1, [x19, #128]\n\
         ldr q2, [x19, #144]\n\
         ldr q3, [x19, #160]\n\
         ldr q4, [x19, #176]\n\
         ldr q5, [x19, #192]\n\
         ldr q6, [x19, #208]\n\
         ldr q7, [x19, #224]\n\
         ldr x0, [x19, #48]\n\
         ldr x1, [x19, #56]\n\
         ldr x2, [x19, #64]\n\
         ldr x3, [x19, #72]\n\
         ldr x4, [x19, #80]\n\
         ldr x5, [x19, #88]\n\
         ldr x6, [x19, #96]\n\
         ldr x7, [x19, #104]\n\
         ldr x8, [x19, #32]\n\
         blr x16\n\
         stp x0, x1, [x19, #240]\n\
         str q0, [x19, #256]\n\
         str q1, [x19, #272]\n\
         str q2, [x19, #288]\n\
         str q3, [x19, #304]\n\
         mov sp, x29\n\
         ldp x19, x20, [sp, #16]\n\
         ldp x29, x30, [sp], #32\n\
         .cfi_def_cfa sp, 0\n\
         ret\n",
    );
    target_function_footer(symbol, abi, &mut source);
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
        AssemblyFunctionLinkage::ExternalHidden,
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
         ld t0, 8(s1)\n\
         fld fa0, 112(s1)\n\
         fld fa1, 128(s1)\n\
         fld fa2, 144(s1)\n\
         fld fa3, 160(s1)\n\
         fld fa4, 176(s1)\n\
         fld fa5, 192(s1)\n\
         fld fa6, 208(s1)\n\
         fld fa7, 224(s1)\n\
         ld a0, 48(s1)\n\
         ld a1, 56(s1)\n\
         ld a2, 64(s1)\n\
         ld a3, 72(s1)\n\
         ld a4, 80(s1)\n\
         ld a5, 88(s1)\n\
         ld a6, 96(s1)\n\
         ld a7, 104(s1)\n\
         jalr t0\n\
         sd a0, 240(s1)\n\
         sd a1, 248(s1)\n\
         fsd fa0, 256(s1)\n\
         fsd fa1, 272(s1)\n\
         fsd fa2, 288(s1)\n\
         fsd fa3, 304(s1)\n\
         addi sp, s0, -32\n\
         ld s2, 0(sp)\n\
         ld s1, 8(sp)\n\
         ld s0, 16(sp)\n\
         ld ra, 24(sp)\n\
         addi sp, sp, 32\n\
         .cfi_def_cfa_offset 0\n\
         ret\n",
    );
    target_function_footer(symbol, AbiIdentity::RiscvLp64d, &mut source);
    GeneratedAssembly::new(
        "call-helper",
        source,
        vec![symbol.to_owned()],
        vec![LogicalDebugLocation::generated(1)],
    )
}

/// Precomputed information needed to render a public variadic entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariadicEntryPlan {
    pub public_symbol: String,
    pub hidden_body_symbol: String,
    pub linkage: AssemblyFunctionLinkage,
    pub fixed_gp_used: u8,
    pub fixed_sse_used: u8,
    /// Offset from the incoming stack argument area to the first variadic item.
    pub overflow_arg_offset: u32,
    /// Number of integer result registers populated by the hidden body.
    pub gp_results: u8,
    /// Number of SSE result registers populated by the hidden body.
    pub xmm_results: u8,
    /// Echo the saved incoming structure-return pointer in `%rax`.
    pub hidden_return: bool,
    pub logical_line: u32,
}

impl VariadicEntryPlan {
    fn validate(&self, abi: AbiIdentity) -> Result<(), LinkError> {
        validate_symbol(&self.public_symbol)?;
        validate_symbol(&self.hidden_body_symbol)?;
        if self.public_symbol == self.hidden_body_symbol {
            return Err(artifact_error(
                "a variadic entry and its hidden body must use distinct symbols",
            ));
        }
        let gp_limit = if abi == AbiIdentity::SysvAmd64Lp64 {
            6
        } else {
            8
        };
        if self.fixed_gp_used > gp_limit || self.fixed_sse_used > 8 {
            return Err(artifact_error(
                "variadic fixed-prefix register counts exceed the target register areas",
            ));
        }
        if !self.overflow_arg_offset.is_multiple_of(8) {
            return Err(artifact_error(
                "the variadic overflow argument offset must be eight-byte aligned",
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
                "a variadic entry result exceeds the target register banks",
            ));
        }
        if self.hidden_return && (self.gp_results != 0 || self.xmm_results != 0) {
            return Err(artifact_error(
                "an indirect variadic result cannot also request register results",
            ));
        }
        Ok(())
    }
}

/// Renders the public assembly entry for a compiler-defined variadic function.
pub fn render_variadic_entry(plan: &VariadicEntryPlan) -> Result<GeneratedAssembly, LinkError> {
    render_target_variadic_entry(plan, AbiIdentity::SysvAmd64Lp64)
}

/// Renders a public variadic entry for the selected CCC ABI identity.
pub fn render_target_variadic_entry(
    plan: &VariadicEntryPlan,
    abi: AbiIdentity,
) -> Result<GeneratedAssembly, LinkError> {
    plan.validate(abi)?;

    match abi {
        AbiIdentity::SysvAmd64Lp64 => render_sysv_amd64_variadic_entry(plan),
        AbiIdentity::Aapcs64Lp64 | AbiIdentity::DarwinArm64 => {
            render_arm64_variadic_entry(plan, abi)
        }
        AbiIdentity::RiscvLp64d => render_riscv64_variadic_entry(plan),
    }
}

fn render_sysv_amd64_variadic_entry(
    plan: &VariadicEntryPlan,
) -> Result<GeneratedAssembly, LinkError> {
    let mut source = String::new();
    assembly_prelude(&mut source);
    function_header(&plan.public_symbol, plan.linkage, &mut source);
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
    source.push_str("subq $256, %rsp\n");
    // `%al` is caller-owned variadic metadata and must be captured before any
    // instruction reuses `%rax`. A zero count lets the entry avoid reading
    // undefined incoming XMM registers while the pre-zeroed save area remains
    // deterministic.
    source.push_str("movzbl %al, %r11d\n");
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
    // Initialize result storage deterministically after consuming the `%al`
    // snapshot used by the register-save guard.
    source.push_str(
        "xorq %r11, %r11\n\
         movq %r11, 208(%rsp)\n\
         movq %r11, 216(%rsp)\n\
         movq %r11, 224(%rsp)\n\
         movq %r11, 232(%rsp)\n\
         movq %r11, 240(%rsp)\n\
         movq %r11, 248(%rsp)\n\
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
    }
    source.push_str("leave\n.cfi_def_cfa %rsp, 8\nret\n");
    function_footer(&plan.public_symbol, &mut source);
    GeneratedAssembly::new(
        format!("variadic-entry-{}", short_stem(&plan.public_symbol)),
        source,
        vec![plan.public_symbol.clone()],
        vec![LogicalDebugLocation::generated(plan.logical_line.max(1))],
    )
}

fn render_arm64_variadic_entry(
    plan: &VariadicEntryPlan,
    abi: AbiIdentity,
) -> Result<GeneratedAssembly, LinkError> {
    let mut source = String::new();
    target_function_header(&plan.public_symbol, plan.linkage, abi, &mut source);
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
         ldp x29, x30, [sp], #16\n\
         .cfi_def_cfa sp, 0\n\
         ret\n",
    );
    target_function_footer(&plan.public_symbol, abi, &mut source);
    GeneratedAssembly::new(
        format!("variadic-entry-{}", short_stem(&plan.public_symbol)),
        source,
        vec![plan.public_symbol.clone()],
        vec![LogicalDebugLocation::generated(plan.logical_line.max(1))],
    )
}

fn render_riscv64_variadic_entry(plan: &VariadicEntryPlan) -> Result<GeneratedAssembly, LinkError> {
    let abi = AbiIdentity::RiscvLp64d;
    let mut source = String::new();
    target_function_header(&plan.public_symbol, plan.linkage, abi, &mut source);
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
         fsd fa0, 288(sp)\n\
         fsd fa1, 304(sp)\n\
         fsd fa2, 320(sp)\n\
         fsd fa3, 336(sp)\n\
         fsd fa4, 352(sp)\n\
         fsd fa5, 368(sp)\n\
         fsd fa6, 384(sp)\n\
         fsd fa7, 400(sp)\n",
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
    target_function_footer(&plan.public_symbol, abi, &mut source);
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
    fn protocol_layouts_are_exact() {
        assert_eq!(size_of::<BridgeFrameV1>(), 256);
        assert_eq!(offset_of!(BridgeFrameV1, gp_slots), 32);
        assert_eq!(offset_of!(BridgeFrameV1, xmm_slots), 80);
        assert_eq!(offset_of!(BridgeFrameV1, gp_result_slots), 208);
        assert_eq!(offset_of!(BridgeFrameV1, xmm_result_slots), 224);
        assert_eq!(size_of::<VaStateV1>(), 208);
        assert_eq!(offset_of!(VaStateV1, gp_offset), 8);
        assert_eq!(offset_of!(VaStateV1, register_save_area), 32);
    }

    #[test]
    fn variadic_sse_count_saturates_at_eight() {
        for (actual, expected) in [(0, 0), (1, 1), (8, 8), (9, 8), (u8::MAX, 8)] {
            let mut frame = BridgeFrameV1::zeroed(0, 0);
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
    fn call_helper_has_unconditional_slots_and_a_last_al_write() {
        let assembly = render_generic_call_helper("__ccc_call_helper_test").unwrap();
        let source = assembly.source();
        for index in 0..8 {
            assert!(source.contains(&format!("%xmm{index}")));
        }
        let al_instruction = "movzbl 26(%r12), %eax";
        let al = source.find(al_instruction).unwrap();
        let call = source.find("call *%r11").unwrap();
        assert!(al < call);
        let between = &source[al + al_instruction.len()..call];
        assert!(!between.contains("%rax"));
        assert!(!between.contains("%eax\n"));
        assert!(source.contains(".cfi_startproc"));
        assert!(source.contains(".note.GNU-stack"));
        assert!(!source.contains(".file"));
        assert!(!source.contains(".loc"));
        assert!(!source.contains("ud2"));
        assert!(!source.contains("ldmxcsr"));
        assert!(!source.contains("fldcw"));
        assert!(!source.contains("cld"));
    }

    #[test]
    fn variadic_entry_builds_the_public_va_list_view() {
        let assembly = render_variadic_entry(&VariadicEntryPlan {
            public_symbol: "consume".to_owned(),
            hidden_body_symbol: "__ccc_variadic_body_test".to_owned(),
            linkage: AssemblyFunctionLinkage::ExternalHidden,
            fixed_gp_used: 2,
            fixed_sse_used: 1,
            overflow_arg_offset: 16,
            gp_results: 1,
            xmm_results: 0,
            hidden_return: false,
            logical_line: 27,
        })
        .unwrap();
        let source = assembly.source();
        assert!(source.contains("subq $256, %rsp"));
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
    fn variadic_entry_rejects_conflicting_indirect_and_register_results() {
        let error = render_variadic_entry(&VariadicEntryPlan {
            public_symbol: "consume".to_owned(),
            hidden_body_symbol: "hidden".to_owned(),
            linkage: AssemblyFunctionLinkage::Internal,
            fixed_gp_used: 2,
            fixed_sse_used: 0,
            overflow_arg_offset: 0,
            gp_results: 1,
            xmm_results: 0,
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
    fn variadic_entry_retains_internal_source_linkage() {
        let assembly = render_variadic_entry(&VariadicEntryPlan {
            public_symbol: "local_consume".to_owned(),
            hidden_body_symbol: "__ccc_variadic_body_local".to_owned(),
            linkage: AssemblyFunctionLinkage::Internal,
            fixed_gp_used: 0,
            fixed_sse_used: 0,
            overflow_arg_offset: 0,
            gp_results: 0,
            xmm_results: 0,
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
            let assembly = render_variadic_entry(&VariadicEntryPlan {
                public_symbol: "consume".to_owned(),
                hidden_body_symbol: "__ccc_variadic_body_visibility".to_owned(),
                linkage,
                fixed_gp_used: 0,
                fixed_sse_used: 0,
                overflow_arg_offset: 0,
                gp_results: 1,
                xmm_results: 0,
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
    fn variadic_entry_guards_each_xmm_save_with_the_incoming_al_bound() {
        let assembly = render_variadic_entry(&VariadicEntryPlan {
            public_symbol: "consume".to_owned(),
            hidden_body_symbol: "__ccc_variadic_body_al_bound".to_owned(),
            linkage: AssemblyFunctionLinkage::ExternalDefault,
            fixed_gp_used: 0,
            fixed_sse_used: 0,
            overflow_arg_offset: 0,
            gp_results: 1,
            xmm_results: 0,
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
