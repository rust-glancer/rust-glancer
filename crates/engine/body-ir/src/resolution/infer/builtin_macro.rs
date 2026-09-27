//! Type facts for compiler-provided expression macros.
//!
//! Def-map marks resolved compiler builtin macro definitions before Body IR lowers them. Body
//! resolution only needs the conservative type fact for that lowered builtin expression, so this
//! module keeps the compiler-known type construction out of the general expression walker.

use rg_ir_model::{BuiltinMacroExprKind, Mutability};
use rg_item_tree::LangItem;
use rg_ty::{
    PrimitiveTy, UnsignedIntTy,
    solver::{AdtTy, DefId, List, Ty},
};

use super::BodyInference;

impl<'s, 'query, D, I> BodyInference<'s, 'query, D, I> {
    /// Return the type supplied by a recognized compiler builtin.
    pub(super) fn builtin_macro_ty(&self, kind: BuiltinMacroExprKind) -> Ty<'s> {
        match kind {
            BuiltinMacroExprKind::Cfg => self.cx.primitive(PrimitiveTy::Bool),
            BuiltinMacroExprKind::Column | BuiltinMacroExprKind::Line => self
                .cx
                .primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U32)),
            BuiltinMacroExprKind::Concat
            | BuiltinMacroExprKind::Env
            | BuiltinMacroExprKind::File
            | BuiltinMacroExprKind::IncludeStr
            | BuiltinMacroExprKind::ModulePath
            | BuiltinMacroExprKind::Stringify => self.static_str_ty(),
            BuiltinMacroExprKind::IncludeBytes => self.cx.reference(
                Mutability::Shared,
                self.cx.slice(
                    self.cx
                        .primitive(PrimitiveTy::UnsignedInt(UnsignedIntTy::U8)),
                ),
            ),
            // These macros use compiler language identities. Looking up `core::...` in the
            // caller's scope would let a local `mod core {}` change their result types.
            // Without the declarations, there is no nominal type identity to return.
            BuiltinMacroExprKind::FormatArgs | BuiltinMacroExprKind::FormatArgsNl => {
                let Some(def) = self
                    .context
                    .item_lookup_query()
                    .lang_type(LangItem::FormatArguments)
                else {
                    return self.cx.unknown();
                };
                self.cx.adt(AdtTy {
                    def,
                    args: self.cx.unknown_args(DefId::Adt(def)),
                })
            }
            BuiltinMacroExprKind::OptionEnv => {
                let Some(def) = self.context.item_lookup_query().lang_type(LangItem::Option) else {
                    return self.cx.unknown();
                };
                self.cx.adt(AdtTy {
                    def,
                    args: List::new(self.cx, &[self.static_str_ty().into()]),
                })
            }
        }
    }

    fn static_str_ty(&self) -> Ty<'s> {
        self.cx
            .reference(Mutability::Shared, self.cx.primitive(PrimitiveTy::Str))
    }
}
