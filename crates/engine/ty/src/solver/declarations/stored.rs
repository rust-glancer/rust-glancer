//! Owned declaration parts shared between operations in one immutable lexical context.
//!
//! These entries use owned Rust Glancer types. For `fn id<T>(value: T) -> T`, the signature keeps `T`;
//! the inference variable chosen for a particular call never enters this cache. Each operation
//! imports the parts it needs into its own temporary solver storage.
//!
//! Each part has a separate entry so reading a cached impl header needs no predicates. Name
//! lookup can therefore use that header while the predicates are still being lowered.

use std::sync::Arc;

use rg_ir_model::{FunctionRef, ImplRef, TypeAliasRef, TypeDefRef};
use rustc_type_ir::data_structures::HashMap;

use super::DeclarationMetadata;
use crate::{Clause, Ty, signature, solver::DefId};

#[derive(Default)]
pub(crate) struct StoredDeclarations {
    pub metadata: HashMap<DefId, Arc<DeclarationMetadata>>,
    pub impl_headers: HashMap<ImplRef, Arc<signature::ImplHeader>>,
    pub functions: HashMap<FunctionRef, Arc<signature::CallableSignature>>,
    pub predicates: HashMap<DefId, Arc<[Clause]>>,
    pub bounds: HashMap<DefId, Arc<[Clause]>>,
    pub alias_values: HashMap<TypeAliasRef, Arc<Ty>>,
    pub fields: HashMap<TypeDefRef, Arc<[Ty]>>,
}
