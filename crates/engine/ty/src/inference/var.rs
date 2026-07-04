use rg_std::MemorySize;

/// Body-local identity of an inference variable slot.
///
/// The id is meaningful only inside the `InferenceTable` that allocated it. It may appear inside
/// `Ty` while inference is running, but finalization must erase it before any type is persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
pub struct InferVarId(u32);

impl InferVarId {
    pub(crate) fn from_slot_index(index: usize) -> Self {
        Self(
            index
                .try_into()
                .expect("one body should not allocate more than u32::MAX inference variables"),
        )
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// Inference variable class.
///
/// Numeric literals use narrower variables because an unsuffixed `1` should default to `i32`
/// while an unconstrained ordinary type variable should become `Ty::Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, MemorySize)]
pub enum InferVarKind {
    Type,
    Integer,
    Float,
}
