use super::function::scalar_type;
use super::*;

pub(super) fn requested_alignment(
    requested: Option<u64>,
    natural: u64,
) -> Result<u64, CodegenError> {
    let align = requested.unwrap_or(natural).max(natural);
    if !align.is_power_of_two() {
        return Err(error(format!(
            "requested object alignment {align} is not a power of two"
        )));
    }
    Ok(align)
}

pub(super) fn define_strings(
    module: &gir::FullModule,
    declarations: &Declarations,
    object_module: &mut ObjectModule,
) -> Result<(), CodegenError> {
    for string in &module.strings {
        let bytes = encode_string(string)?;
        let mut description = DataDescription::new();
        description.define(bytes.into_boxed_slice());
        let alignment = match string.encoding {
            gir::StringEncoding::Ordinary | gir::StringEncoding::Utf8 => 1,
            gir::StringEncoding::Utf16 => 2,
            gir::StringEncoding::Wide | gir::StringEncoding::Utf32 => 4,
        };
        description.set_align(alignment);
        let id = declarations
            .strings
            .get(&string.id.0)
            .copied()
            .ok_or_else(|| error(format!("string {} was not declared", string.id.0)))?;
        object_module
            .define_data(id, &description)
            .map_err(module_error)?;
    }
    Ok(())
}

fn encode_string(string: &gir::FullString) -> Result<Vec<u8>, CodegenError> {
    let unit_bytes = string_unit_bytes(string.encoding);
    let mut bytes = Vec::with_capacity(string.code_units.len() * unit_bytes);
    for unit in &string.code_units {
        match unit_bytes {
            1 => {
                bytes.push(u8::try_from(*unit).map_err(|_| {
                    error(format!("string code unit {unit} does not fit in one byte"))
                })?)
            }
            2 => bytes.extend_from_slice(
                &u16::try_from(*unit)
                    .map_err(|_| error(format!("string code unit {unit} does not fit in UTF-16")))?
                    .to_le_bytes(),
            ),
            4 => bytes.extend_from_slice(&unit.to_le_bytes()),
            _ => unreachable!(),
        }
    }
    Ok(bytes)
}

pub(super) fn string_unit_bytes(encoding: gir::StringEncoding) -> usize {
    match encoding {
        gir::StringEncoding::Ordinary | gir::StringEncoding::Utf8 => 1,
        gir::StringEncoding::Utf16 => 2,
        gir::StringEncoding::Wide | gir::StringEncoding::Utf32 => 4,
    }
}

pub(super) fn define_globals(
    module: &gir::FullModule,
    config: &EffectiveCompilationConfig,
    declarations: &Declarations,
    object_module: &mut ObjectModule,
) -> Result<(), CodegenError> {
    for global in &module.globals {
        if global.emission.definition == ObjectDefinitionPolicy::Declaration
            || (global.emission.definition == ObjectDefinitionPolicy::TentativeCommon
                && global.linkage == CLinkage::External
                && global.duration != StorageDuration::Thread
                && global.emission.tls.is_none()
                && config.target.triple.binary_format == BinaryFormat::Elf)
        {
            continue;
        }
        (|| -> Result<(), CodegenError> {
            let layout = object_layout(&module.types, global.ty, config)?;
            let align = requested_alignment(global.emission.requested_alignment, layout.align)?;
            let mut description = DataDescription::new();
            if let Some(initializer) = &global.initializer {
                let mut writer = InitializerWriter::new(module, config, initializer, layout.size)?;
                writer.write_root()?;
                let (bytes, relocations) = writer.finish();
                description.define(bytes.into_boxed_slice());
                apply_relocations(&mut description, &relocations, declarations, object_module)?;
            } else {
                description.define_zeroinit(
                    usize::try_from(layout.size)
                        .map_err(|_| error("global object is too large for the object writer"))?,
                );
            }
            description.set_align(align);
            if let Some(section) = &global.emission.section {
                backend::set_custom_data_section(&mut description, section);
            }
            let declaration = declarations
                .globals
                .get(&global.id.0)
                .copied()
                .ok_or_else(|| error(format!("data object {} was not declared", global.id.0)))?;
            object_module
                .define_data(declaration.id, &description)
                .map_err(module_error)?;
            Ok(())
        })()
        .map_err(|error| error.with_span_if_none(global.span))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PendingRelocationTarget {
    Object(u32),
    Function(u32),
    String(u32),
}

struct PendingRelocation {
    offset: u32,
    target: PendingRelocationTarget,
    addend: i64,
    kind: gir::RelocationKind,
}

#[derive(Clone, Copy)]
struct Placement {
    offset: u64,
    bitfield: Option<gir::BitfieldDescriptor>,
}

struct InitializerWriter<'a> {
    module: &'a gir::FullModule,
    config: &'a EffectiveCompilationConfig,
    graph: &'a gir::InitializerGraph,
    bytes: Vec<u8>,
    relocations: Vec<PendingRelocation>,
    active: HashSet<u32>,
}

impl<'a> InitializerWriter<'a> {
    fn new(
        module: &'a gir::FullModule,
        config: &'a EffectiveCompilationConfig,
        graph: &'a gir::InitializerGraph,
        size: u64,
    ) -> Result<Self, CodegenError> {
        Ok(Self {
            module,
            config,
            graph,
            bytes: vec![
                0;
                usize::try_from(size).map_err(|_| error(
                    "initialized object is too large for the object writer"
                ))?
            ],
            relocations: Vec::new(),
            active: HashSet::new(),
        })
    }

    fn write_root(&mut self) -> Result<(), CodegenError> {
        self.write_node(
            self.graph.root,
            Placement {
                offset: 0,
                bitfield: None,
            },
        )
    }

    fn finish(self) -> (Vec<u8>, Vec<PendingRelocation>) {
        (self.bytes, self.relocations)
    }

    fn write_node(
        &mut self,
        id: gir::InitializerNodeId,
        placement: Placement,
    ) -> Result<(), CodegenError> {
        if !self.active.insert(id.0) {
            return Err(error(format!(
                "initializer graph contains a cycle through node {}",
                id.0
            )));
        }
        let node = self
            .graph
            .nodes
            .iter()
            .find(|node| node.id == id)
            .cloned()
            .ok_or_else(|| error(format!("initializer references unknown node {}", id.0)))?;
        match node.kind {
            gir::InitializerNodeKind::Zero => self.zero(node.ty, placement)?,
            gir::InitializerNodeKind::Scalar(value) => {
                self.write_scalar(node.ty, value, placement)?
            }
            gir::InitializerNodeKind::Relocation {
                target,
                addend,
                one_past: _,
                kind,
            } => self.write_relocation(node.ty, target, addend, kind, placement)?,
            gir::InitializerNodeKind::StringData {
                string,
                copy_code_units,
            } => self.write_string(node.ty, string.0, copy_code_units, placement)?,
            gir::InitializerNodeKind::Repeat { element, count } => {
                if placement.bitfield.is_some() {
                    return Err(error("a repeated initializer cannot target a bitfield"));
                }
                let TypeKind::Array(_) = self.module.types.kind(node.ty.ty) else {
                    return Err(error("a repeated initializer must target an array"));
                };
                let layout = object_layout(&self.module.types, node.ty, self.config)?;
                let LayoutShape::Array { length, stride } = layout.shape else {
                    return Err(error("a repeated initializer has no array layout"));
                };
                if count == 0 || count > length {
                    return Err(error(
                        "a repeated initializer count is outside its array bound",
                    ));
                }
                self.zero(node.ty, placement)?;
                for index in 0..count {
                    let offset = placement
                        .offset
                        .checked_add(index.checked_mul(stride).ok_or_else(|| {
                            error("repeated initializer offset multiplication overflow")
                        })?)
                        .ok_or_else(|| error("repeated initializer offset overflow"))?;
                    self.write_node(
                        element,
                        Placement {
                            offset,
                            bitfield: None,
                        },
                    )?;
                }
            }
            gir::InitializerNodeKind::Aggregate(edges) => {
                if placement.bitfield.is_some() {
                    return Err(error("an aggregate initializer cannot target a bitfield"));
                }
                self.zero(node.ty, placement)?;
                for edge in edges {
                    let child = self.placement_for_path(node.ty, placement.offset, &edge.path)?;
                    self.write_node(edge.node, child)?;
                }
            }
        }
        self.active.remove(&id.0);
        Ok(())
    }

    fn zero(&mut self, ty: QualifiedType, placement: Placement) -> Result<(), CodegenError> {
        if let Some(bitfield) = placement.bitfield {
            return self.write_bitfield(placement.offset, bitfield, 0);
        }
        let layout = object_layout(&self.module.types, ty, self.config)?;
        let range = self.range(placement.offset, layout.size)?;
        self.bytes[range].fill(0);
        Ok(())
    }

    fn write_scalar(
        &mut self,
        ty: QualifiedType,
        value: gir::ScalarConstant,
        placement: Placement,
    ) -> Result<(), CodegenError> {
        let raw = scalar_constant_bits(&self.module.types, ty, value, self.config)?;
        if let Some(bitfield) = placement.bitfield {
            return self.write_bitfield(placement.offset, bitfield, raw);
        }
        let layout = object_layout(&self.module.types, ty, self.config)?;
        let size = usize::try_from(layout.size)
            .map_err(|_| error("scalar initializer size does not fit in memory"))?;
        if !matches!(size, 1 | 2 | 4 | 8 | 16) {
            return Err(error(format!(
                "scalar initializer for `{}` has unsupported size {size}",
                self.module.types.display(ty.ty)
            )));
        }
        let range = self.range(placement.offset, layout.size)?;
        let encoded = raw.to_le_bytes();
        self.bytes[range].copy_from_slice(&encoded[..size]);
        Ok(())
    }

    fn write_bitfield(
        &mut self,
        base: u64,
        descriptor: gir::BitfieldDescriptor,
        raw: u128,
    ) -> Result<(), CodegenError> {
        if !matches!(descriptor.storage_size, 1 | 2 | 4 | 8 | 16) {
            return Err(error(format!(
                "bitfield {} uses unsupported storage size {}",
                descriptor.field_index, descriptor.storage_size
            )));
        }
        let unit_bits = u32::try_from(descriptor.storage_size * 8)
            .map_err(|_| error("bitfield storage width overflow"))?;
        if descriptor.width > unit_bits
            || descriptor.bit_offset > unit_bits
            || descriptor.bit_offset + descriptor.width > unit_bits
        {
            return Err(error(format!(
                "bitfield {} has invalid storage geometry",
                descriptor.field_index
            )));
        }
        let offset = base
            .checked_add(descriptor.storage_offset)
            .ok_or_else(|| error("bitfield initializer offset overflow"))?;
        let range = self.range(offset, descriptor.storage_size)?;
        let mut encoded = [0_u8; 16];
        encoded[..range.len()].copy_from_slice(&self.bytes[range.clone()]);
        let mut unit = u128::from_le_bytes(encoded);
        let value_mask = low_mask_u128(descriptor.width);
        let field_mask = value_mask.checked_shl(descriptor.bit_offset).unwrap_or(0);
        unit = (unit & !field_mask)
            | ((raw & value_mask)
                .checked_shl(descriptor.bit_offset)
                .unwrap_or(0));
        self.bytes[range].copy_from_slice(&unit.to_le_bytes()[..descriptor.storage_size as usize]);
        Ok(())
    }

    fn write_string(
        &mut self,
        ty: QualifiedType,
        string_id: u32,
        copy_code_units: u64,
        placement: Placement,
    ) -> Result<(), CodegenError> {
        if placement.bitfield.is_some() {
            return Err(error("a string initializer cannot target a bitfield"));
        }
        let string = self
            .module
            .strings
            .iter()
            .find(|string| string.id.0 == string_id)
            .ok_or_else(|| error(format!("initializer references unknown string {string_id}")))?;
        let encoded = encode_string(string)?;
        let layout = object_layout(&self.module.types, ty, self.config)?;
        let copy_size = copy_code_units
            .checked_mul(string_unit_bytes(string.encoding) as u64)
            .ok_or_else(|| error("string initializer byte count overflow"))?;
        if copy_code_units > string.code_units.len() as u64 || copy_size > layout.size {
            return Err(error(format!(
                "string initializer requests {copy_code_units} code units for an object of {} bytes",
                layout.size
            )));
        }
        self.zero(ty, placement)?;
        let copy_size = usize::try_from(copy_size)
            .map_err(|_| error("string initializer is too large for the object writer"))?;
        let range = self.range(placement.offset, copy_size as u64)?;
        self.bytes[range].copy_from_slice(&encoded[..copy_size]);
        Ok(())
    }

    fn write_relocation(
        &mut self,
        ty: QualifiedType,
        target: gir::RelocationTarget,
        addend: i128,
        kind: gir::RelocationKind,
        placement: Placement,
    ) -> Result<(), CodegenError> {
        if placement.bitfield.is_some() {
            return Err(error("an address relocation cannot target a bitfield"));
        }
        let layout = object_layout(&self.module.types, ty, self.config)?;
        if layout.size != 8 {
            return Err(error(format!(
                "address relocation has non-pointer size {}",
                layout.size
            )));
        }
        let range = self.range(placement.offset, layout.size)?;
        self.bytes[range].fill(0);
        let addend = i64::try_from(addend)
            .map_err(|_| error("initializer relocation addend does not fit in 64 bits"))?;
        let target = match target {
            gir::RelocationTarget::Object(id) => PendingRelocationTarget::Object(id.0),
            gir::RelocationTarget::Function(id) => PendingRelocationTarget::Function(id.0),
            gir::RelocationTarget::String(id) => PendingRelocationTarget::String(id.0),
        };
        self.relocations.push(PendingRelocation {
            offset: u32::try_from(placement.offset)
                .map_err(|_| error("initializer relocation offset exceeds object limits"))?,
            target,
            addend,
            kind,
        });
        Ok(())
    }

    fn placement_for_path(
        &self,
        root: QualifiedType,
        base: u64,
        path: &[gir::InitializerPath],
    ) -> Result<Placement, CodegenError> {
        let mut current = root;
        let mut offset = base;
        let mut bitfield = None;
        for (path_index, component) in path.iter().enumerate() {
            if bitfield.is_some() {
                return Err(error("an initializer path continues through a bitfield"));
            }
            match component {
                gir::InitializerPath::Index(index) => {
                    let TypeKind::Array(array) = self.module.types.kind(current.ty) else {
                        return Err(error(format!(
                            "initializer index is applied to `{}`",
                            self.module.types.display(current.ty)
                        )));
                    };
                    let layout = object_layout(&self.module.types, current, self.config)?;
                    let LayoutShape::Array { length, stride } = layout.shape else {
                        return Err(error("array initializer has no array layout"));
                    };
                    if *index >= length {
                        return Err(error(format!(
                            "initializer index {index} is outside array bound {length}"
                        )));
                    }
                    offset = offset
                        .checked_add(index.checked_mul(stride).ok_or_else(|| {
                            error("initializer array offset multiplication overflow")
                        })?)
                        .ok_or_else(|| error("initializer array offset overflow"))?;
                    current = array.element;
                }
                gir::InitializerPath::Field {
                    index,
                    name,
                    bitfield: descriptor,
                } => {
                    let TypeKind::Record(record_id) = self.module.types.kind(current.ty) else {
                        return Err(error(format!(
                            "initializer field is applied to `{}`",
                            self.module.types.display(current.ty)
                        )));
                    };
                    let definition = self
                        .module
                        .types
                        .record(*record_id)
                        .and_then(|record| record.fields.as_ref())
                        .ok_or_else(|| error("initializer references an incomplete record"))?;
                    let field = definition.get(*index).ok_or_else(|| {
                        error(format!("initializer references unknown field {index}"))
                    })?;
                    if let Some(expected) = name
                        && field.name.as_deref() != Some(expected.as_str())
                    {
                        return Err(error(format!(
                            "initializer field {index} name does not match `{expected}`"
                        )));
                    }
                    let layout = object_layout(&self.module.types, current, self.config)?;
                    let LayoutShape::Record(record_layout) = layout.shape else {
                        return Err(error("record initializer has no record layout"));
                    };
                    let field_layout = record_layout
                        .fields
                        .get(*index)
                        .ok_or_else(|| error(format!("record layout has no field {index}")))?;
                    offset = offset
                        .checked_add(field_layout.offset)
                        .ok_or_else(|| error("initializer field offset overflow"))?;
                    if let Some(descriptor) = descriptor {
                        if path_index + 1 != path.len() {
                            return Err(error("an initializer path continues through a bitfield"));
                        }
                        bitfield = Some(*descriptor);
                    }
                    current = field.ty;
                }
            }
        }
        Ok(Placement { offset, bitfield })
    }

    fn range(&self, offset: u64, size: u64) -> Result<std::ops::Range<usize>, CodegenError> {
        let end = offset
            .checked_add(size)
            .ok_or_else(|| error("initializer range overflow"))?;
        let start = usize::try_from(offset)
            .map_err(|_| error("initializer offset does not fit in memory"))?;
        let end =
            usize::try_from(end).map_err(|_| error("initializer end does not fit in memory"))?;
        if end > self.bytes.len() {
            return Err(error(format!(
                "initializer range {start}..{end} exceeds object size {}",
                self.bytes.len()
            )));
        }
        Ok(start..end)
    }
}

fn apply_relocations(
    description: &mut DataDescription,
    relocations: &[PendingRelocation],
    declarations: &Declarations,
    object_module: &ObjectModule,
) -> Result<(), CodegenError> {
    for relocation in relocations {
        match relocation.target {
            PendingRelocationTarget::Object(raw) => {
                if relocation.kind == gir::RelocationKind::ThreadLocalAddress {
                    return Err(error(
                        "a static initializer cannot contain a thread-local object address",
                    ));
                }
                let target =
                    declarations.globals.get(&raw).copied().ok_or_else(|| {
                        error(format!("relocation references unknown data {raw}"))
                    })?;
                let reference = object_module.declare_data_in_data(target.id, description);
                description.write_data_addr(relocation.offset, reference, relocation.addend);
            }
            PendingRelocationTarget::String(raw) => {
                if relocation.kind == gir::RelocationKind::ThreadLocalAddress {
                    return Err(error(
                        "a string address cannot use a thread-local relocation",
                    ));
                }
                let target =
                    declarations.strings.get(&raw).copied().ok_or_else(|| {
                        error(format!("relocation references unknown string {raw}"))
                    })?;
                let reference = object_module.declare_data_in_data(target, description);
                description.write_data_addr(relocation.offset, reference, relocation.addend);
            }
            PendingRelocationTarget::Function(raw) => {
                if relocation.kind != gir::RelocationKind::FunctionAddress {
                    return Err(error(
                        "a function target uses a non-function relocation kind",
                    ));
                }
                if relocation.addend != 0 {
                    return Err(error(
                        "function-address relocations with nonzero addends are unsupported",
                    ));
                }
                let target = declarations.functions.get(&raw).copied().ok_or_else(|| {
                    error(format!("relocation references unknown function {raw}"))
                })?;
                let reference = object_module.declare_func_in_data(target, description);
                description.write_function_addr(relocation.offset, reference);
            }
        }
    }
    Ok(())
}

pub(super) fn scalar_constant_bits(
    types: &TypeStore,
    ty: QualifiedType,
    value: gir::ScalarConstant,
    config: &EffectiveCompilationConfig,
) -> Result<u128, CodegenError> {
    if types.builtin_type(ty.ty) == Some(BuiltinType::Bool) {
        return Ok(u128::from(match value {
            gir::ScalarConstant::Signed(value) => value != 0,
            gir::ScalarConstant::Unsigned(value) => value != 0,
            gir::ScalarConstant::Floating(value) => value != 0.0,
            gir::ScalarConstant::LongDouble(value) => !value.is_zero(),
            gir::ScalarConstant::NullPointer => false,
        }));
    }
    match value {
        gir::ScalarConstant::Signed(value) => Ok(value as u128),
        gir::ScalarConstant::Unsigned(value) => Ok(value),
        gir::ScalarConstant::Floating(value) => match scalar_type(types, ty, config)? {
            ir::types::I16 if types.builtin_type(ty.ty) == Some(BuiltinType::Float16) => {
                Ok(u128::from(f64_to_f16_bits(value)))
            }
            ir::types::F32 => Ok(u128::from((value as f32).to_bits())),
            ir::types::F64 => Ok(u128::from(value.to_bits())),
            _ => Err(error(format!(
                "floating initializer targets non-floating type `{}`",
                types.display(ty.ty)
            ))),
        },
        gir::ScalarConstant::LongDouble(value) => {
            if types.builtin_type(ty.ty) != Some(BuiltinType::LongDouble) {
                return Err(error(format!(
                    "long-double initializer targets `{}`",
                    types.display(ty.ty)
                )));
            }
            if value.format != config.target.data_layout.long_double_format {
                return Err(error(
                    "long-double initializer format differs from the target",
                ));
            }
            Ok(value.bits())
        }
        gir::ScalarConstant::NullPointer => match types.try_kind(ty.ty) {
            Some(TypeKind::Pointer(_)) => Ok(0),
            _ => Err(error(format!(
                "null pointer initializer targets `{}`",
                types.display(ty.ty)
            ))),
        },
    }
}

pub(super) fn low_mask_u128(width: u32) -> u128 {
    if width >= 128 {
        u128::MAX
    } else if width == 0 {
        0
    } else {
        (1_u128 << width) - 1
    }
}
