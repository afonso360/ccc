//! Target data-layout facts shared by semantic analysis and object emission.

/// Byte order used for scalar storage and bitfield allocation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ByteOrder {
    Little,
    Big,
}

/// Target scalar categories whose signedness does not affect their layout.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TargetScalarKind {
    Bool,
    Char,
    Short,
    Int,
    Long,
    LongLong,
    Float16,
    Float,
    Double,
    LongDouble,
    Pointer,
}

/// Size and required alignment in bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScalarLayout {
    pub size: u64,
    pub align: u64,
}

impl ScalarLayout {
    pub const fn new(size: u64, align: u64) -> Self {
        Self { size, align }
    }
}

/// Direction in which fields consume bits inside a storage unit.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BitfieldOrder {
    LeastSignificantFirst,
    MostSignificantFirst,
}

/// How a target allocates C bitfields.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BitfieldLayoutPolicy {
    pub order: BitfieldOrder,
    /// Whether one bitfield may straddle two declared-type storage units.
    pub may_cross_storage_units: bool,
    /// Whether adjacent bitfields with different declared integer types may
    /// continue at the next available bit within a compatible allocation
    /// unit.
    pub coalesce_different_declared_types: bool,
    /// Whether a packing constraint makes adjacent bitfields consume bits
    /// contiguously instead of observing natural allocation-unit boundaries.
    pub packed_fields_are_contiguous: bool,
    /// Whether an unnamed zero-width bitfield aligns the next field to the
    /// declared bitfield type.
    pub zero_width_uses_declared_alignment: bool,
}

/// Alignment constraints applied by packing pragmas and attributes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PackingPolicy {
    /// Maximum alignment allowed for an individual field. `None` preserves
    /// the field's natural alignment.
    pub maximum_field_alignment: Option<u64>,
    /// Minimum alignment retained by the complete record.
    pub minimum_record_alignment: u64,
}

impl PackingPolicy {
    pub const NATIVE: Self = Self {
        maximum_field_alignment: None,
        minimum_record_alignment: 1,
    };

    pub const PACKED: Self = Self {
        maximum_field_alignment: Some(1),
        minimum_record_alignment: 1,
    };

    pub const fn with_maximum_field_alignment(maximum: u64) -> Self {
        Self {
            maximum_field_alignment: Some(maximum),
            minimum_record_alignment: 1,
        }
    }

    pub const fn with_minimum_record_alignment(mut self, minimum: u64) -> Self {
        self.minimum_record_alignment = minimum;
        self
    }

    pub const fn is_valid(self) -> bool {
        self.minimum_record_alignment.is_power_of_two()
            && match self.maximum_field_alignment {
                Some(maximum) => maximum.is_power_of_two(),
                None => true,
            }
    }

    pub const fn field_alignment(self, natural: u64) -> u64 {
        match self.maximum_field_alignment {
            Some(maximum) if maximum < natural => maximum,
            _ => natural,
        }
    }

    pub const fn record_alignment(self, fields: u64) -> u64 {
        if self.minimum_record_alignment > fields {
            self.minimum_record_alignment
        } else {
            fields
        }
    }

    /// Combines target defaults with a record-local packing constraint.
    pub const fn combine(self, local: Self) -> Self {
        let maximum_field_alignment =
            match (self.maximum_field_alignment, local.maximum_field_alignment) {
                (Some(left), Some(right)) => Some(if left < right { left } else { right }),
                (Some(value), None) | (None, Some(value)) => Some(value),
                (None, None) => None,
            };
        let minimum_record_alignment =
            if self.minimum_record_alignment > local.minimum_record_alignment {
                self.minimum_record_alignment
            } else {
                local.minimum_record_alignment
            };
        Self {
            maximum_field_alignment,
            minimum_record_alignment,
        }
    }
}

impl Default for PackingPolicy {
    fn default() -> Self {
        Self::NATIVE
    }
}

/// Immutable data-layout defaults for an enabled target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetDataLayout {
    pub byte_order: ByteOrder,
    pub char_is_signed: bool,
    pub bool_width: u8,
    pub bool_align: u8,
    pub char_width: u8,
    pub char_align: u8,
    pub short_width: u8,
    pub short_align: u8,
    pub int_width: u8,
    pub int_align: u8,
    pub long_width: u8,
    pub long_align: u8,
    pub long_long_width: u8,
    pub long_long_align: u8,
    pub pointer_width: u8,
    pub pointer_align: u8,
    pub float_width: u8,
    pub float_align: u8,
    pub double_width: u8,
    pub double_align: u8,
    pub long_double_width: u8,
    pub long_double_align: u8,
    pub wchar_width: u8,
    pub wchar_is_signed: bool,
    pub wint_width: u8,
    pub wint_is_signed: bool,
    pub bitfields: BitfieldLayoutPolicy,
    pub default_packing: PackingPolicy,
}

impl TargetDataLayout {
    /// Returns the storage layout for a target scalar category.
    pub const fn scalar(self, kind: TargetScalarKind) -> ScalarLayout {
        let (width, align) = match kind {
            TargetScalarKind::Bool => (self.bool_width, self.bool_align),
            TargetScalarKind::Char => (self.char_width, self.char_align),
            TargetScalarKind::Short => (self.short_width, self.short_align),
            TargetScalarKind::Int => (self.int_width, self.int_align),
            TargetScalarKind::Long => (self.long_width, self.long_align),
            TargetScalarKind::LongLong => (self.long_long_width, self.long_long_align),
            TargetScalarKind::Float16 => (16, 2),
            TargetScalarKind::Float => (self.float_width, self.float_align),
            TargetScalarKind::Double => (self.double_width, self.double_align),
            TargetScalarKind::LongDouble => (self.long_double_width, self.long_double_align),
            TargetScalarKind::Pointer => (self.pointer_width, self.pointer_align),
        };
        ScalarLayout::new((width as u64).div_ceil(8), align as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_policies_cap_fields_and_retain_record_alignment() {
        assert_eq!(PackingPolicy::PACKED.field_alignment(16), 1);
        let aligned =
            PackingPolicy::with_maximum_field_alignment(4).with_minimum_record_alignment(8);
        assert!(aligned.is_valid());
        assert_eq!(aligned.field_alignment(16), 4);
        assert_eq!(aligned.record_alignment(4), 8);
        assert!(!PackingPolicy::with_maximum_field_alignment(3).is_valid());
    }
}
