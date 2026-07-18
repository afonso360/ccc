use std::collections::HashSet;
use std::fmt;

use ccc_target::{BitfieldOrder, EffectiveCompilationConfig, PackingPolicy, TargetScalarKind};

use crate::{
    ArrayLength, ArrayType, BuiltinType, EnumId, Field, RecordId, RecordKind, TypeId, TypeKind,
    TypeStore, VariableLengthId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeLayout {
    pub size: u64,
    pub align: u64,
    pub shape: LayoutShape,
}

impl TypeLayout {
    pub const fn scalar(size: u64, align: u64, builtin: BuiltinType) -> Self {
        Self {
            size,
            align,
            shape: LayoutShape::Builtin(builtin),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutShape {
    Builtin(BuiltinType),
    Pointer,
    Array { length: u64, stride: u64 },
    Enum { id: EnumId, underlying: TypeId },
    Record(RecordLayout),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordLayout {
    pub id: RecordId,
    pub kind: RecordKind,
    pub fields: Vec<FieldLayout>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldLayout {
    pub index: usize,
    pub offset: u64,
    pub size: u64,
    pub align: u64,
    pub bitfield: Option<BitfieldLayout>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitfieldLayout {
    pub storage_offset: u64,
    pub storage_size: u64,
    pub storage_align: u64,
    pub bit_offset: u32,
    pub width: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    UnknownType(TypeId),
    UnsizedType(TypeId),
    IncompleteArray(TypeId),
    VariableLengthArray {
        ty: TypeId,
        bound: VariableLengthId,
    },
    IncompleteRecord(RecordId),
    IncompleteEnum(EnumId),
    RecursiveType(TypeId),
    InvalidPacking(PackingPolicy),
    InvalidAlignment(u64),
    NonIntegerBitfield {
        record: RecordId,
        field: usize,
        ty: TypeId,
    },
    NamedZeroWidthBitfield {
        record: RecordId,
        field: usize,
    },
    BitfieldTooWide {
        record: RecordId,
        field: usize,
        width: u32,
        maximum: u32,
    },
    UnsupportedCrossingBitfields,
    FlexibleArrayNotLast {
        record: RecordId,
        field: usize,
    },
    Overflow(TypeId),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownType(ty) => write!(formatter, "unknown type {}", ty.0),
            Self::UnsizedType(ty) => write!(formatter, "type {} has no object size", ty.0),
            Self::IncompleteArray(ty) => write!(formatter, "array type {} is incomplete", ty.0),
            Self::VariableLengthArray { ty, bound } => write!(
                formatter,
                "array type {} has runtime bound {}",
                ty.0, bound.0
            ),
            Self::IncompleteRecord(id) => write!(formatter, "record {} is incomplete", id.0),
            Self::IncompleteEnum(id) => write!(formatter, "enum {} is incomplete", id.0),
            Self::RecursiveType(ty) => {
                write!(formatter, "type {} contains itself by value", ty.0)
            }
            Self::InvalidPacking(policy) => write!(
                formatter,
                "invalid packing policy with field cap {:?} and record alignment {}",
                policy.maximum_field_alignment, policy.minimum_record_alignment
            ),
            Self::InvalidAlignment(align) => write!(formatter, "invalid alignment {align}"),
            Self::NonIntegerBitfield { record, field, ty } => write!(
                formatter,
                "field {field} of record {} uses non-integer bitfield type {}",
                record.0, ty.0
            ),
            Self::NamedZeroWidthBitfield { record, field } => write!(
                formatter,
                "named field {field} of record {} has zero width",
                record.0
            ),
            Self::BitfieldTooWide {
                record,
                field,
                width,
                maximum,
            } => write!(
                formatter,
                "bitfield {field} of record {} has width {width}, maximum {maximum}",
                record.0
            ),
            Self::UnsupportedCrossingBitfields => {
                write!(formatter, "cross-storage-unit bitfields are unsupported")
            }
            Self::FlexibleArrayNotLast { record, field } => write!(
                formatter,
                "incomplete array field {field} of record {} is not a final struct member",
                record.0
            ),
            Self::Overflow(ty) => write!(formatter, "layout of type {} overflows", ty.0),
        }
    }
}

impl std::error::Error for LayoutError {}

impl TypeStore {
    /// Computes a structured layout using the effective target configuration.
    pub fn layout_of(
        &self,
        id: TypeId,
        config: &EffectiveCompilationConfig,
    ) -> Result<TypeLayout, LayoutError> {
        let packing = match self.try_kind(id) {
            Some(TypeKind::Record(record)) => self
                .record(*record)
                .map_or(PackingPolicy::NATIVE, |record| record.packing),
            _ => PackingPolicy::NATIVE,
        };
        let key = crate::store::LayoutCacheKey {
            ty: id,
            target: config.target.data_layout,
            packing,
        };
        if let Some((_, result)) = self
            .layout_cache
            .borrow()
            .iter()
            .find(|(candidate, _)| *candidate == key)
        {
            return result.clone();
        }
        let result = LayoutEngine {
            store: self,
            config,
            active: HashSet::new(),
        }
        .layout(id);
        self.layout_cache.borrow_mut().push((key, result.clone()));
        result
    }
}

struct LayoutEngine<'a> {
    store: &'a TypeStore,
    config: &'a EffectiveCompilationConfig,
    active: HashSet<TypeId>,
}

impl LayoutEngine<'_> {
    fn layout(&mut self, id: TypeId) -> Result<TypeLayout, LayoutError> {
        let kind = self
            .store
            .try_kind(id)
            .cloned()
            .ok_or(LayoutError::UnknownType(id))?;
        match kind {
            TypeKind::Builtin(kind) => self.builtin_layout(kind),
            TypeKind::Pointer(_) => {
                let target = self.config.target.scalar_layout(TargetScalarKind::Pointer);
                self.validate_scalar(target.size, target.align)?;
                Ok(TypeLayout {
                    size: target.size,
                    align: target.align,
                    shape: LayoutShape::Pointer,
                })
            }
            TypeKind::Array(array) => self.array_layout(id, array),
            TypeKind::Function(_) => Err(LayoutError::UnsizedType(id)),
            TypeKind::Enum(enum_id) => self.enum_layout(enum_id),
            TypeKind::Record(record_id) => self.record_layout(id, record_id),
        }
    }

    fn builtin_layout(&self, builtin: BuiltinType) -> Result<TypeLayout, LayoutError> {
        let target_kind = match builtin {
            BuiltinType::Void => return Err(LayoutError::UnsizedType(TypeId::VOID)),
            BuiltinType::Bool => TargetScalarKind::Bool,
            BuiltinType::Char | BuiltinType::SignedChar | BuiltinType::UnsignedChar => {
                TargetScalarKind::Char
            }
            BuiltinType::Short | BuiltinType::UnsignedShort => TargetScalarKind::Short,
            BuiltinType::Int | BuiltinType::UnsignedInt => TargetScalarKind::Int,
            BuiltinType::Long | BuiltinType::UnsignedLong => TargetScalarKind::Long,
            BuiltinType::LongLong | BuiltinType::UnsignedLongLong => TargetScalarKind::LongLong,
            BuiltinType::Float => TargetScalarKind::Float,
            BuiltinType::Float16 => TargetScalarKind::Float16,
            BuiltinType::Double => TargetScalarKind::Double,
            BuiltinType::LongDouble => TargetScalarKind::LongDouble,
            BuiltinType::Int128 | BuiltinType::UnsignedInt128 => {
                return Ok(TypeLayout::scalar(16, 16, builtin));
            }
        };
        let target = self.config.target.scalar_layout(target_kind);
        self.validate_scalar(target.size, target.align)?;
        Ok(TypeLayout::scalar(target.size, target.align, builtin))
    }

    fn array_layout(&mut self, id: TypeId, array: ArrayType) -> Result<TypeLayout, LayoutError> {
        let length = match array.length {
            ArrayLength::Incomplete => return Err(LayoutError::IncompleteArray(id)),
            ArrayLength::Variable(bound) | ArrayLength::UnspecifiedVariable(bound) => {
                return Err(LayoutError::VariableLengthArray { ty: id, bound });
            }
            ArrayLength::Constant(length) => length,
        };
        let element = self.layout(array.element.ty)?;
        let size = element
            .size
            .checked_mul(length)
            .ok_or(LayoutError::Overflow(id))?;
        Ok(TypeLayout {
            size,
            align: element.align,
            shape: LayoutShape::Array {
                length,
                stride: element.size,
            },
        })
    }

    fn enum_layout(&mut self, id: EnumId) -> Result<TypeLayout, LayoutError> {
        let definition = self
            .store
            .enumeration(id)
            .ok_or(LayoutError::IncompleteEnum(id))?;
        let body = definition
            .body
            .as_ref()
            .ok_or(LayoutError::IncompleteEnum(id))?;
        let underlying = self.layout(body.underlying)?;
        Ok(TypeLayout {
            size: underlying.size,
            align: underlying.align,
            shape: LayoutShape::Enum {
                id,
                underlying: body.underlying,
            },
        })
    }

    fn record_layout(&mut self, ty: TypeId, id: RecordId) -> Result<TypeLayout, LayoutError> {
        if !self.active.insert(ty) {
            return Err(LayoutError::RecursiveType(ty));
        }
        let result = self.record_layout_inner(ty, id);
        self.active.remove(&ty);
        result
    }

    fn record_layout_inner(&mut self, ty: TypeId, id: RecordId) -> Result<TypeLayout, LayoutError> {
        let definition = self
            .store
            .record(id)
            .cloned()
            .ok_or(LayoutError::IncompleteRecord(id))?;
        let fields = definition.fields.ok_or(LayoutError::IncompleteRecord(id))?;
        let packing = self
            .config
            .target
            .data_layout
            .default_packing
            .combine(definition.packing);
        if !packing.is_valid() {
            return Err(LayoutError::InvalidPacking(packing));
        }
        if self
            .config
            .target
            .data_layout
            .bitfields
            .may_cross_storage_units
        {
            return Err(LayoutError::UnsupportedCrossingBitfields);
        }

        let (size, align, field_layouts) = match definition.kind {
            RecordKind::Struct => self.struct_fields(ty, id, &fields, packing)?,
            RecordKind::Union => self.union_fields(ty, id, &fields, packing)?,
        };
        Ok(TypeLayout {
            size,
            align,
            shape: LayoutShape::Record(RecordLayout {
                id,
                kind: definition.kind,
                fields: field_layouts,
            }),
        })
    }

    fn struct_fields(
        &mut self,
        record_ty: TypeId,
        record: RecordId,
        fields: &[Field],
        packing: PackingPolicy,
    ) -> Result<(u64, u64, Vec<FieldLayout>), LayoutError> {
        let mut cursor = 0_u64;
        let mut extent = 0_u64;
        let mut record_align = 1_u64;
        let mut layouts = Vec::with_capacity(fields.len());
        let mut active_bitfield: Option<ActiveBitfield> = None;

        for (index, field) in fields.iter().enumerate() {
            if let Some(bitfield) = field.bitfield {
                let storage = self.bitfield_storage(record, index, field)?;
                let storage_align = packing.field_alignment(storage.align);
                self.validate_alignment(storage_align)?;
                let maximum = self.bitfield_maximum(field.ty.ty, &storage);
                if bitfield.width > maximum {
                    return Err(LayoutError::BitfieldTooWide {
                        record,
                        field: index,
                        width: bitfield.width,
                        maximum,
                    });
                }
                if bitfield.width == 0 {
                    if field.name.is_some() {
                        return Err(LayoutError::NamedZeroWidthBitfield {
                            record,
                            field: index,
                        });
                    }
                    active_bitfield = None;
                    let barrier_align = if self
                        .config
                        .target
                        .data_layout
                        .bitfields
                        .zero_width_uses_declared_alignment
                    {
                        storage.align
                    } else {
                        1
                    };
                    self.validate_alignment(barrier_align)?;
                    cursor = align_up(extent, barrier_align)?;
                    extent = extent.max(cursor);
                    layouts.push(FieldLayout {
                        index,
                        offset: cursor,
                        size: 0,
                        align: barrier_align,
                        bitfield: Some(BitfieldLayout {
                            storage_offset: cursor,
                            storage_size: storage.size,
                            storage_align: barrier_align,
                            bit_offset: 0,
                            width: 0,
                        }),
                    });
                    continue;
                }

                record_align = record_align.max(storage_align);
                let policy = self.config.target.data_layout.bitfields;
                let cursor_bits = cursor
                    .checked_mul(8)
                    .ok_or(LayoutError::Overflow(record_ty))?;
                let mut next_bit = active_bitfield
                    .map(|unit| unit.next_bit)
                    .unwrap_or(cursor_bits);
                if active_bitfield.is_some_and(|unit| {
                    !policy.coalesce_different_declared_types
                        && (unit.storage_size != storage.size
                            || unit.storage_align != storage_align)
                }) {
                    next_bit = align_up(cursor, storage_align)?
                        .checked_mul(8)
                        .ok_or(LayoutError::Overflow(record_ty))?;
                }

                let declared_bits = u64::from(storage_bits(&storage)?);
                let packed_contiguous =
                    policy.packed_fields_are_contiguous && storage_align < storage.align;
                let (start_bit, storage_offset, access_size, used_in_access) = if packed_contiguous
                {
                    let storage_offset = next_bit / 8;
                    let used_in_access = next_bit % 8;
                    let required_size = used_in_access
                        .checked_add(u64::from(bitfield.width))
                        .ok_or(LayoutError::Overflow(record_ty))?
                        .div_ceil(8);
                    let widened_size = required_size.next_power_of_two();
                    let access_size = if widened_size <= storage.size {
                        widened_size
                    } else {
                        required_size
                    };
                    (next_bit, storage_offset, access_size, used_in_access)
                } else {
                    let mut unit_start = next_bit / declared_bits * declared_bits;
                    let used = next_bit - unit_start;
                    let start_bit = if used + u64::from(bitfield.width) <= declared_bits {
                        next_bit
                    } else {
                        unit_start = unit_start
                            .checked_add(declared_bits)
                            .ok_or(LayoutError::Overflow(record_ty))?;
                        unit_start
                    };
                    (
                        start_bit,
                        unit_start / 8,
                        storage.size,
                        start_bit - unit_start,
                    )
                };
                let access_bits = access_size
                    .checked_mul(8)
                    .and_then(|bits| u32::try_from(bits).ok())
                    .ok_or(LayoutError::Overflow(record_ty))?;
                let used_in_access =
                    u32::try_from(used_in_access).map_err(|_| LayoutError::Overflow(record_ty))?;
                let bit_offset =
                    bit_offset(policy.order, access_bits, used_in_access, bitfield.width);
                let next_bit = start_bit
                    .checked_add(u64::from(bitfield.width))
                    .ok_or(LayoutError::Overflow(record_ty))?;
                let used_bytes = next_bit.div_ceil(8);
                cursor = cursor.max(used_bytes);
                extent = extent.max(used_bytes);
                active_bitfield = Some(ActiveBitfield {
                    next_bit,
                    storage_size: storage.size,
                    storage_align,
                });
                layouts.push(FieldLayout {
                    index,
                    offset: storage_offset,
                    size: access_size,
                    align: storage_align,
                    bitfield: Some(BitfieldLayout {
                        storage_offset,
                        storage_size: access_size,
                        storage_align,
                        bit_offset,
                        width: bitfield.width,
                    }),
                });
                continue;
            }

            active_bitfield = None;
            let is_last = index + 1 == fields.len();
            let field_layout = self.object_field_layout(record, index, field, is_last, false)?;
            let field_align = effective_field_alignment(field, &field_layout, packing)?;
            self.validate_alignment(field_align)?;
            record_align = record_align.max(field_align);
            let offset = align_up(cursor, field_align)?;
            cursor = offset
                .checked_add(field_layout.size)
                .ok_or(LayoutError::Overflow(record_ty))?;
            extent = extent.max(cursor);
            layouts.push(FieldLayout {
                index,
                offset,
                size: field_layout.size,
                align: field_align,
                bitfield: None,
            });
        }

        let align = packing.record_alignment(record_align);
        self.validate_alignment(align)?;
        Ok((align_up(extent, align)?, align, layouts))
    }

    fn union_fields(
        &mut self,
        record_ty: TypeId,
        record: RecordId,
        fields: &[Field],
        packing: PackingPolicy,
    ) -> Result<(u64, u64, Vec<FieldLayout>), LayoutError> {
        let mut size = 0_u64;
        let mut record_align = 1_u64;
        let mut layouts = Vec::with_capacity(fields.len());
        for (index, field) in fields.iter().enumerate() {
            if let Some(bitfield) = field.bitfield {
                let storage = self.bitfield_storage(record, index, field)?;
                let storage_align = packing.field_alignment(storage.align);
                self.validate_alignment(storage_align)?;
                let maximum = self.bitfield_maximum(field.ty.ty, &storage);
                if bitfield.width > maximum {
                    return Err(LayoutError::BitfieldTooWide {
                        record,
                        field: index,
                        width: bitfield.width,
                        maximum,
                    });
                }
                if bitfield.width == 0 {
                    if field.name.is_some() {
                        return Err(LayoutError::NamedZeroWidthBitfield {
                            record,
                            field: index,
                        });
                    }
                    let barrier_align = if self
                        .config
                        .target
                        .data_layout
                        .bitfields
                        .zero_width_uses_declared_alignment
                    {
                        storage.align
                    } else {
                        1
                    };
                    layouts.push(FieldLayout {
                        index,
                        offset: 0,
                        size: 0,
                        align: barrier_align,
                        bitfield: Some(BitfieldLayout {
                            storage_offset: 0,
                            storage_size: storage.size,
                            storage_align: barrier_align,
                            bit_offset: 0,
                            width: 0,
                        }),
                    });
                    continue;
                }

                record_align = record_align.max(storage_align);
                let packed = self
                    .config
                    .target
                    .data_layout
                    .bitfields
                    .packed_fields_are_contiguous
                    && storage_align < storage.align;
                let access_size = if packed {
                    u64::from(bitfield.width).div_ceil(8)
                } else {
                    storage.size
                };
                size = size.max(access_size);
                let bits = access_size
                    .checked_mul(8)
                    .and_then(|bits| u32::try_from(bits).ok())
                    .ok_or(LayoutError::Overflow(record_ty))?;
                layouts.push(FieldLayout {
                    index,
                    offset: 0,
                    size: access_size,
                    align: storage_align,
                    bitfield: Some(BitfieldLayout {
                        storage_offset: 0,
                        storage_size: access_size,
                        storage_align,
                        bit_offset: bit_offset(
                            self.config.target.data_layout.bitfields.order,
                            bits,
                            0,
                            bitfield.width,
                        ),
                        width: bitfield.width,
                    }),
                });
                continue;
            }

            let field_layout = self.object_field_layout(record, index, field, false, true)?;
            let field_align = effective_field_alignment(field, &field_layout, packing)?;
            self.validate_alignment(field_align)?;
            record_align = record_align.max(field_align);
            size = size.max(field_layout.size);
            layouts.push(FieldLayout {
                index,
                offset: 0,
                size: field_layout.size,
                align: field_align,
                bitfield: None,
            });
        }
        let align = packing.record_alignment(record_align);
        self.validate_alignment(align)?;
        Ok((
            align_up(size, align).map_err(|_| LayoutError::Overflow(record_ty))?,
            align,
            layouts,
        ))
    }

    fn object_field_layout(
        &mut self,
        record: RecordId,
        index: usize,
        field: &Field,
        is_last: bool,
        in_union: bool,
    ) -> Result<TypeLayout, LayoutError> {
        if let Some(TypeKind::Array(ArrayType {
            element,
            length: ArrayLength::Incomplete,
        })) = self.store.try_kind(field.ty.ty).cloned()
        {
            if !is_last || in_union {
                return Err(LayoutError::FlexibleArrayNotLast {
                    record,
                    field: index,
                });
            }
            let element = self.layout(element.ty)?;
            return Ok(TypeLayout {
                size: 0,
                align: element.align,
                shape: LayoutShape::Array {
                    length: 0,
                    stride: element.size,
                },
            });
        }
        self.layout(field.ty.ty)
    }

    fn bitfield_storage(
        &mut self,
        record: RecordId,
        index: usize,
        field: &Field,
    ) -> Result<TypeLayout, LayoutError> {
        if !self.store.is_integer(field.ty.ty) {
            return Err(LayoutError::NonIntegerBitfield {
                record,
                field: index,
                ty: field.ty.ty,
            });
        }
        self.layout(field.ty.ty)
    }

    fn bitfield_maximum(&self, ty: TypeId, storage: &TypeLayout) -> u32 {
        if self.store.builtin_type(ty) == Some(BuiltinType::Bool) {
            1
        } else {
            u32::try_from(storage.size.saturating_mul(8)).unwrap_or(u32::MAX)
        }
    }

    fn validate_scalar(&self, size: u64, align: u64) -> Result<(), LayoutError> {
        if size == 0 {
            return Err(LayoutError::InvalidAlignment(align));
        }
        self.validate_alignment(align)
    }

    fn validate_alignment(&self, align: u64) -> Result<(), LayoutError> {
        if align.is_power_of_two() {
            Ok(())
        } else {
            Err(LayoutError::InvalidAlignment(align))
        }
    }
}

fn effective_field_alignment(
    field: &Field,
    layout: &TypeLayout,
    packing: PackingPolicy,
) -> Result<u64, LayoutError> {
    let packed = packing.field_alignment(layout.align);
    let Some(requested) = field.requested_alignment else {
        return Ok(packed);
    };
    if !requested.is_power_of_two() || requested < layout.align {
        return Err(LayoutError::InvalidAlignment(requested));
    }
    Ok(packed.max(requested))
}

#[derive(Clone, Copy)]
struct ActiveBitfield {
    next_bit: u64,
    storage_size: u64,
    storage_align: u64,
}

fn storage_bits(storage: &TypeLayout) -> Result<u32, LayoutError> {
    u32::try_from(storage.size.saturating_mul(8))
        .map_err(|_| LayoutError::InvalidAlignment(storage.align))
}

fn bit_offset(order: BitfieldOrder, storage: u32, used: u32, width: u32) -> u32 {
    if width == 0 {
        return 0;
    }
    match order {
        BitfieldOrder::LeastSignificantFirst => used,
        BitfieldOrder::MostSignificantFirst => storage - used - width,
    }
}

fn align_up(value: u64, align: u64) -> Result<u64, LayoutError> {
    if !align.is_power_of_two() {
        return Err(LayoutError::InvalidAlignment(align));
    }
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or(LayoutError::InvalidAlignment(align))
}
