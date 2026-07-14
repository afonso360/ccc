//! System V call-frame information for Cranelift-compiled functions.

use cranelift_codegen::Context;
use cranelift_codegen::isa::{TargetIsa, unwind::UnwindInfo};
use cranelift_module::FuncId;
use cranelift_object::ObjectProduct;
use gimli::constants::{DW_EH_PE_pcrel, DW_EH_PE_sdata4, DwEhPe};
use gimli::write::{
    Address, EhFrame, EndianVec, FrameTable, RelocateWriter, Relocation, RelocationTarget,
};
use object::write::{Relocation as ObjectRelocation, StandardSection, SymbolId, SymbolSection};
use object::{BinaryFormat, RelocationEncoding, RelocationFlags, RelocationKind};

/// Collects the unwind description that `cranelift-object` 0.132 does not
/// preserve when it copies compiled function bytes into its object writer.
pub(super) struct UnwindEmitter {
    cie: gimli::write::CommonInformationEntry,
    functions: Vec<(FuncId, cranelift_codegen::isa::unwind::systemv::UnwindInfo)>,
}

impl UnwindEmitter {
    pub(super) fn new(isa: &dyn TargetIsa) -> Result<Self, String> {
        let mut cie = isa.create_systemv_cie().ok_or_else(|| {
            "the configured backend cannot create System V unwind data".to_owned()
        })?;
        // A signed PC-relative word is the conventional ELF encoding. Unlike
        // an absolute pointer, it remains linkable in position-independent
        // executables without a dynamic relocation in `.eh_frame`.
        cie.fde_address_encoding = pcrel_sdata4();
        Ok(Self {
            cie,
            functions: Vec::new(),
        })
    }

    pub(super) fn record_function(
        &mut self,
        function: FuncId,
        context: &Context,
        isa: &dyn TargetIsa,
    ) -> Result<(), String> {
        let compiled = context.compiled_code().ok_or_else(|| {
            format!(
                "function {} has no compiled machine code for unwind emission",
                function.as_u32()
            )
        })?;
        let info = compiled
            .create_unwind_info(isa)
            .map_err(|error| format!("failed to create System V unwind data: {error}"))?
            .ok_or_else(|| format!("function {} has no System V unwind data", function.as_u32()))?;
        let UnwindInfo::SystemV(info) = info else {
            return Err(format!(
                "function {} produced non-System-V unwind data",
                function.as_u32()
            ));
        };
        self.functions.push((function, info));
        Ok(())
    }

    pub(super) fn emit(self, product: &mut ObjectProduct) -> Result<(), String> {
        if self.functions.is_empty() {
            return Ok(());
        }
        if product.object.format() != BinaryFormat::Elf {
            return Err("System V unwind emission requires an ELF object".to_owned());
        }

        let mut frame_table = FrameTable::default();
        let cie = frame_table.add_cie(self.cie);
        let mut targets = Vec::with_capacity(self.functions.len());
        for (index, (function, info)) in self.functions.into_iter().enumerate() {
            targets.push(section_target(product, function)?);
            frame_table.add_fde(
                cie,
                info.to_fde(Address::Symbol {
                    symbol: index,
                    addend: 0,
                }),
            );
        }

        let mut eh_frame = EhFrame(EhFrameSection::default());
        frame_table
            .write_eh_frame(&mut eh_frame)
            .map_err(|error| format!("failed to encode `.eh_frame`: {error}"))?;
        let EhFrame(EhFrameSection {
            writer,
            relocations,
        }) = eh_frame;

        let section = product.object.section_id(StandardSection::EhFrame);
        product.object.section_mut(section).flags = object::SectionFlags::Elf {
            sh_flags: u64::from(object::elf::SHF_ALLOC),
        };
        let section_offset = product
            .object
            .append_section_data(section, &writer.into_vec(), 8);
        for relocation in relocations {
            let RelocationTarget::Symbol(index) = relocation.target else {
                return Err("unexpected section-relative `.eh_frame` relocation".to_owned());
            };
            let target = targets.get(index).ok_or_else(|| {
                format!("`.eh_frame` relocation references unknown target {index}")
            })?;
            if relocation.size != 4 || relocation.eh_pe != Some(pcrel_sdata4()) {
                return Err(format!(
                    "unsupported `.eh_frame` relocation encoding: size={} eh_pe={:?}",
                    relocation.size, relocation.eh_pe
                ));
            }
            let offset = section_offset
                .checked_add(u64::try_from(relocation.offset).map_err(|_| {
                    "`.eh_frame` relocation offset does not fit in an object offset".to_owned()
                })?)
                .ok_or_else(|| "`.eh_frame` relocation offset overflowed".to_owned())?;
            let addend = target
                .addend
                .checked_add(relocation.addend)
                .ok_or_else(|| "`.eh_frame` relocation addend overflowed".to_owned())?;
            product
                .object
                .add_relocation(
                    section,
                    ObjectRelocation {
                        offset,
                        symbol: target.symbol,
                        addend,
                        flags: RelocationFlags::Generic {
                            kind: RelocationKind::Relative,
                            encoding: RelocationEncoding::Generic,
                            size: 32,
                        },
                    },
                )
                .map_err(|error| format!("failed to record `.eh_frame` relocation: {error}"))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct SectionTarget {
    symbol: SymbolId,
    addend: i64,
}

fn section_target(product: &mut ObjectProduct, function: FuncId) -> Result<SectionTarget, String> {
    let function_symbol = product.function_symbol(function);
    let (section, value) = {
        let symbol = product.object.symbol(function_symbol);
        let SymbolSection::Section(section) = symbol.section else {
            return Err(format!(
                "function {} is not defined in an object section",
                function.as_u32()
            ));
        };
        (section, symbol.value)
    };
    let addend = i64::try_from(value).map_err(|_| {
        format!(
            "function {} has an object offset that exceeds ELF relocation limits",
            function.as_u32()
        )
    })?;
    Ok(SectionTarget {
        symbol: product.object.section_symbol(section),
        addend,
    })
}

fn pcrel_sdata4() -> DwEhPe {
    DwEhPe(DW_EH_PE_pcrel.0 | DW_EH_PE_sdata4.0)
}

struct EhFrameSection {
    writer: EndianVec<gimli::LittleEndian>,
    relocations: Vec<Relocation>,
}

impl Default for EhFrameSection {
    fn default() -> Self {
        Self {
            writer: EndianVec::new(gimli::LittleEndian),
            relocations: Vec::new(),
        }
    }
}

impl RelocateWriter for EhFrameSection {
    type Writer = EndianVec<gimli::LittleEndian>;

    fn writer(&self) -> &Self::Writer {
        &self.writer
    }

    fn writer_mut(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }

    fn relocate(&mut self, relocation: Relocation) {
        self.relocations.push(relocation);
    }
}
